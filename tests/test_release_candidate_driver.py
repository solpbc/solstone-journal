# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import hashlib
import inspect
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tomllib
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from io import BytesIO
from pathlib import Path
from typing import Any

import pytest
from packaging.version import InvalidVersion, Version

import scripts.check_rust_release_manifest as checker
import scripts.check_wheel_contents as wheel_checker
import scripts.release_build_host as release_build_host
import scripts.release_candidate_driver as driver
import scripts.release_ledger as ledger
import scripts.release_tool_pins as pins
from scripts.check_wheel_contents import (
    PARAKEET_HELPER_MEMBER,
    core_wheel_script_members,
)
from scripts.release_build_host import BuildHostResult, SourceBundle
from scripts.release_nvattest_proof import CHALLENGE_RE
from scripts.transparency_core import collect_candidate_parts, snapshot_candidate
from tests.helpers.release_candidate_fixtures import (
    DRIFT_MODEL_VERSION,
    LOCK_SHA,
    MACOS_CORE,
    MACOS_HELPER,
    MACOS_ONNXRUNTIME,
    MACOS_SPEAKERS_ANALYZE,
    SOURCE_COMMIT,
    SPEAKERS_ANALYZE_LICENSE_BYTES,
    SPEAKERS_ANALYZE_RUNTIME_BYTES,
    SPEAKERS_ANALYZE_THIRD_PARTY_NOTICE_BYTES,
    write_core_unsupported_tombstone_record,
)
from tests.helpers.release_candidate_fixtures import (
    env as _env,
)
from tests.helpers.release_candidate_fixtures import (
    macos_wheel_names as _macos_wheel_names,
)
from tests.helpers.release_candidate_fixtures import (
    recover as _recover,
)
from tests.helpers.release_candidate_fixtures import (
    repo as _repo,
)
from tests.helpers.release_candidate_fixtures import (
    services as _services,
)
from tests.helpers.release_candidate_fixtures import (
    write_macos_host_outputs as _write_macos_host_outputs,
)
from tests.helpers.release_wheel_fixtures import (
    write_core_wheel,
    write_platform_base_wheel,
)

PRIOR_RETAINED_VERSION = "1.0.13"
assert PRIOR_RETAINED_VERSION != checker._current_version()
assert not PRIOR_RETAINED_VERSION.startswith(checker._current_version())
assert not checker._current_version().startswith(PRIOR_RETAINED_VERSION)


@pytest.fixture(autouse=True)
def _patch_speakers_analyze_fixture_hashes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patched_targets = {}
    for target, spec in tuple(wheel_checker.SPEAKERS_ANALYZE_TARGETS.items()):
        runtime = (
            MACOS_ONNXRUNTIME
            if target == "macos-arm64"
            else SPEAKERS_ANALYZE_RUNTIME_BYTES
        )
        notices = (
            replace(
                spec.notices[0],
                sha256=hashlib.sha256(SPEAKERS_ANALYZE_LICENSE_BYTES).hexdigest(),
            ),
            replace(
                spec.notices[1],
                sha256=hashlib.sha256(
                    SPEAKERS_ANALYZE_THIRD_PARTY_NOTICE_BYTES
                ).hexdigest(),
            ),
        )
        patched_targets[target] = replace(
            spec,
            runtime_sha256=hashlib.sha256(runtime).hexdigest(),
            notices=notices,
        )
    for module in (
        wheel_checker,
        sys.modules.get("check_wheel_contents"),
    ):
        if module is None:
            continue
        for target, spec in patched_targets.items():
            monkeypatch.setitem(
                module.SPEAKERS_ANALYZE_TARGETS,
                target,
                spec,
            )


def _local_dist_names_for_build_argv(
    argv: Sequence[str], *, include_models: bool
) -> set[str]:
    args = tuple(argv)
    expected = driver._expected_local_dist_names(include_models=include_models)
    if args == ("uv", "build", "--package", "solstone-core", "--sdist"):
        return {
            name
            for name in expected
            if name.startswith("solstone_core-") and name.endswith(".tar.gz")
        }
    if args == (
        "uv",
        "build",
        "--package",
        driver.SPEAKERS_ANALYZE_WORKSPACE_PACKAGE,
        "--wheel",
    ):
        return {
            name
            for name in expected
            if name.startswith("solstone_core_speakers_analyze-")
            and name.endswith(".whl")
        }
    if len(args) == 4 and args[:3] == ("uv", "build", "--package"):
        package = args[3]
        prefix = f"{package.replace('-', '_')}-"
        return {name for name in expected if name.startswith(prefix)}
    if (
        len(args) == 5
        and args[:3] == ("uv", "build", "--package")
        and args[4] == "--wheel"
    ):
        prefix = f"{args[3].replace('-', '_')}-"
        return {
            name
            for name in expected
            if name.startswith(prefix) and name.endswith(".whl")
        }
    return set()


def _write_fake_core_sdist(root: Path, archive: Path) -> None:
    version = archive.name.removeprefix("solstone_core-").removesuffix(".tar.gz")
    source_members = ["crates/solstone-core", "crates/solstone-core-sol"]
    source_manifest = (
        f'[workspace]\nmembers = {json.dumps(source_members)}\nresolver = "3"\n'
    )
    (root / "core" / "crates" / "solstone-core").mkdir(parents=True, exist_ok=True)
    (root / "core" / "crates" / "solstone-core-sol").mkdir(parents=True, exist_ok=True)
    (root / "core" / "crates" / "solstone-core" / "src").mkdir(
        parents=True,
        exist_ok=True,
    )
    (root / "core" / "crates" / "solstone-core-sol" / "src").mkdir(
        parents=True,
        exist_ok=True,
    )
    (root / "core" / "Cargo.toml").write_text(source_manifest, encoding="utf-8")
    (root / "core" / "crates" / "solstone-core" / "Cargo.toml").write_text(
        f'[package]\nname = "solstone-core"\nversion = "{version}"\n',
        encoding="utf-8",
    )
    (root / "core" / "crates" / "solstone-core-sol" / "Cargo.toml").write_text(
        f'[package]\nname = "solstone-core-sol"\nversion = "{version}"\n',
        encoding="utf-8",
    )
    (root / "core" / "crates" / "solstone-core" / "src" / "main.rs").write_text(
        "fn main() {}\n",
        encoding="utf-8",
    )
    (root / "core" / "crates" / "solstone-core-sol" / "src" / "lib.rs").write_text(
        "",
        encoding="utf-8",
    )
    sdist_manifest = (
        '[workspace]\nmembers = ["crates/solstone-core", "crates/solstone-core-sol"]\n'
        'resolver = "3"\n'
    ).encode()
    sdist_lock = (
        "version = 4\n\n"
        f'[[package]]\nname = "solstone-core"\nversion = "{version}"\n\n'
        f'[[package]]\nname = "solstone-core-sol"\nversion = "{version}"\n'
    ).encode()
    archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, mode="w:gz") as target:
        for name, data in (
            (f"solstone_core-{version}/core/Cargo.toml", sdist_manifest),
            (f"solstone_core-{version}/core/Cargo.lock", sdist_lock),
        ):
            member = tarfile.TarInfo(name)
            member.size = len(data)
            target.addfile(member, BytesIO(data))


def _fabricate_local_dist_for_build_argv(
    root: Path, argv: Sequence[str], *, include_models: bool
) -> None:
    args = tuple(argv)
    names = _local_dist_names_for_build_argv(argv, include_models=include_models)
    dist = root / "dist"
    dist.mkdir(parents=True, exist_ok=True)
    if args == ("uv", "build", "--package", "solstone-core", "--sdist"):
        archive = dist / next(iter(names))
        _write_fake_core_sdist(root, archive)
        return
    if (
        len(args) == 6
        and args[:2] == ("uv", "build")
        and args[2].startswith("dist/solstone_core-")
        and args[3:] == ("--wheel", "--out-dir", "dist")
    ):
        core_wheels = {
            name
            for name in driver._expected_local_dist_names(include_models=include_models)
            if name.startswith("solstone_core-") and name.endswith(".whl")
        }
        remaining = sorted(name for name in core_wheels if not (dist / name).exists())
        if remaining:
            (dist / remaining[0]).write_bytes(b"package")
        return
    if args == (
        "uv",
        "build",
        "--package",
        driver.SPEAKERS_ANALYZE_WORKSPACE_PACKAGE,
        "--wheel",
    ):
        helper_wheels = {
            name
            for name in driver._expected_local_dist_names(include_models=include_models)
            if name.startswith("solstone_core_speakers_analyze-")
            and name.endswith(".whl")
        }
        remaining = sorted(name for name in helper_wheels if not (dist / name).exists())
        if remaining:
            (dist / remaining[0]).write_bytes(b"package")
        return
    if (
        len(args) == 5
        and args[:3] == ("uv", "build", "--package")
        and args[4] == "--wheel"
    ):
        remaining = sorted(name for name in names if not (dist / name).exists())
        if remaining:
            (dist / remaining[0]).write_bytes(b"package")
        return
    for name in names:
        (dist / name).write_bytes(b"package")


def _prepare_fake_build_root(root: Path) -> None:
    (root / "pyproject.toml").write_text(
        f'[project]\nversion = "{checker._current_version()}"\n',
        encoding="utf-8",
    )


def _is_final_core_wheel_build(
    root: Path, argv: Sequence[str], *, include_models: bool
) -> bool:
    args = tuple(argv)
    if not (
        len(args) == 6
        and args[:2] == ("uv", "build")
        and args[2].startswith("dist/solstone_core-")
        and args[3:] == ("--wheel", "--out-dir", "dist")
    ):
        return False
    expected = {
        name
        for name in driver._expected_local_dist_names(include_models=include_models)
        if name.startswith("solstone_core-") and name.endswith(".whl")
    }
    return all((root / "dist" / name).is_file() for name in expected)


def _ready_paths(root: Path) -> tuple[Path, Path, Path, Path]:
    version = checker._current_version()
    ready_path = root / "dist" / "release-candidate" / version
    payload_staging = ready_path.parent / f"{version}.payload-staging"
    evidence_dir = root / "target" / "release-evidence" / version
    evidence_staging = root / "target" / "release-evidence" / f"{version}.staging"
    return ready_path, payload_staging, evidence_dir, evidence_staging


def _assert_no_ready_cohort(root: Path) -> None:
    ready_path, payload_staging, evidence_dir, evidence_staging = _ready_paths(root)
    assert not ready_path.exists()
    assert not payload_staging.exists()
    assert not evidence_dir.exists()
    assert not evidence_staging.exists()


def _expected_scrubbed_env(
    root: Path, maturin_args: str, ort_target: str | None
) -> dict[str, str]:
    cache_root = root / "target" / "release-zig-cache"
    env = {
        "MATURIN_PEP517_ARGS": maturin_args,
        "PATH": os.environ["PATH"],
        "PYTHONNOUSERSITE": "1",
        "ZIG_GLOBAL_CACHE_DIR": str((cache_root / "zig-global").resolve()),
        "ZIG_LOCAL_CACHE_DIR": str((cache_root / "zig-local").resolve()),
    }
    if ort_target is not None:
        spec = driver.SPEAKERS_ANALYZE_TARGETS[ort_target]
        env["ORT_LIB_PATH"] = str(
            (root / driver.SPEAKERS_ANALYZE_LINK_ROOT_RELATIVE / spec.key).resolve()
        )
        env["ORT_PREFER_DYNAMIC_LINK"] = "true"
    return env


@dataclass(frozen=True)
class TreeSnapshotEntry:
    relative: str
    kind: str
    mode: int
    symlink_target: str | None
    empty_dir: bool
    size: int | None
    sha256: str | None


def _snapshot_kind(mode: int) -> str:
    if stat.S_ISLNK(mode):
        return "symlink"
    if stat.S_ISDIR(mode):
        return "directory"
    if stat.S_ISREG(mode):
        return "regular"
    return "special"


def _structural_snapshot(path: Path) -> tuple[TreeSnapshotEntry, ...]:
    if not path.exists() and not path.is_symlink():
        return ()
    entries: list[TreeSnapshotEntry] = []

    def visit(current: Path, relative: Path) -> None:
        entry = current.lstat()
        kind = _snapshot_kind(entry.st_mode)
        children: list[Path] = []
        symlink_target: str | None = None
        size: int | None = None
        digest: str | None = None
        if kind == "symlink":
            symlink_target = os.readlink(current)
        elif kind == "regular":
            data = current.read_bytes()
            size = len(data)
            digest = hashlib.sha256(data).hexdigest()
        elif kind == "directory":
            children = sorted(current.iterdir(), key=lambda child: child.name)
        entries.append(
            TreeSnapshotEntry(
                relative=relative.as_posix() if relative.parts else ".",
                kind=kind,
                mode=stat.S_IMODE(entry.st_mode),
                symlink_target=symlink_target,
                empty_dir=kind == "directory" and not children,
                size=size,
                sha256=digest,
            )
        )
        if kind == "directory":
            for child in children:
                visit(child, relative / child.name)

    visit(path, Path())
    return tuple(entries)


def _access_spy(
    monkeypatch: pytest.MonkeyPatch, *, denied_path: Path | None = None
) -> list[tuple[Path, int]]:
    original_access = os.access
    recorded: list[tuple[Path, int]] = []

    def access(path: object, mask: int, *args: object, **kwargs: object) -> bool:
        recorded_path = Path(path)
        recorded.append((recorded_path, mask))
        if denied_path is not None and recorded_path == denied_path:
            return False
        return original_access(path, mask, *args, **kwargs)

    monkeypatch.setattr(os, "access", access)
    return recorded


def _enumeration_spy(monkeypatch: pytest.MonkeyPatch) -> list[Path]:
    original_listdir = os.listdir
    original_scandir = os.scandir
    recorded: list[Path] = []

    def record(path: object) -> None:
        if isinstance(path, int):
            return
        try:
            recorded.append(Path(path))
        except TypeError:
            return

    def listdir(path: object = ".") -> list[str]:
        record(path)
        return original_listdir(path)

    def scandir(path: object = ".") -> object:
        record(path)
        return original_scandir(path)

    monkeypatch.setattr(os, "listdir", listdir)
    monkeypatch.setattr(os, "scandir", scandir)
    return recorded


def _same_or_descendant(path: Path, parent: Path) -> bool:
    return path == parent or path.is_relative_to(parent)


def _real_candidate(tmp_path: Path) -> tuple[Path, driver.CandidateReport]:
    root = _repo(tmp_path)
    return root, driver.run_candidate(root, _env(), _services(root))


def _read_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(payload, dict)
    return payload


def _write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.write_bytes(checker.canonical_json_bytes(payload))


def _ledger_path(report: driver.CandidateReport) -> Path:
    return report.evidence_dir / "ledger.json"


def _write_ledger(report: driver.CandidateReport, payload: Mapping[str, Any]) -> str:
    path = _ledger_path(report)
    _write_json(path, payload)
    return driver.file_sha256_size(path)[0]


def _nvattest_receipt_path(report: driver.CandidateReport, target: str) -> Path:
    return report.evidence_dir / "nvattest" / f"{target}.json"


def _proof_receipt_path(report: driver.CandidateReport, target: str) -> Path:
    return report.evidence_dir / "proofs" / f"{target}.json"


def _update_retained_receipt_ledger_sha(
    report: driver.CandidateReport, ledger_sha256: str
) -> None:
    for target in driver.PROOF_TARGETS:
        for path in (
            _proof_receipt_path(report, target),
            _nvattest_receipt_path(report, target),
        ):
            payload = _read_json(path)
            payload["ledger_sha256"] = ledger_sha256
            _write_json(path, payload)


def _retained_schema(
    report: driver.CandidateReport,
) -> tuple[int, ledger.RetainedLedgerSchema]:
    return ledger.resolve_retained_ledger_schema(_read_json(_ledger_path(report)))


def _derive_pre_nvattest_v1_tree(report: driver.CandidateReport) -> None:
    payload = _read_json(_ledger_path(report))
    payload.pop("nvattest")
    payload["schema_version"] = 1
    ledger_sha256 = _write_ledger(report, payload)
    _update_retained_receipt_ledger_sha(report, ledger_sha256)
    shutil.rmtree(report.evidence_dir / "nvattest")
    shutil.rmtree(report.evidence_dir / "support")


def _assert_fails_with_error(root: Path, expected_error: str) -> None:
    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    errors = [failure.error for failure in exc.value.failures]
    assert expected_error in errors, errors


def _reset_service_call_counts(services: driver.CandidateServices) -> None:
    services.reset_call_counts()


def _assert_service_call_counts_zero(services: driver.CandidateServices) -> None:
    counts = dict(services.call_counts)
    assert counts
    assert counts == {name: 0 for name in counts}


_GUARD_PRECEDING_SERVICE_CALLS = frozenset(
    {"git_head", "git_status", "core_lock_sha256", "git_tag_commit"}
)


def _assert_no_post_guard_service_calls(
    services: driver.CandidateServices,
) -> None:
    counts = dict(services.call_counts)
    assert counts
    assert counts.get("clean_outputs", 0) == 0
    unexpected = {
        name: count
        for name, count in counts.items()
        if name not in _GUARD_PRECEDING_SERVICE_CALLS and count != 0
    }
    assert unexpected == {}


def _assert_recovery_failure_preserves_retained_tree(
    root: Path,
    report: driver.CandidateReport,
    expected_error: str,
    services: driver.CandidateServices,
) -> None:
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)
    _reset_service_call_counts(services)

    _assert_fails_with_error(root, expected_error)

    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    _assert_service_call_counts_zero(services)


def _retained_support_declarations(
    report: driver.CandidateReport,
) -> list[dict[str, Any]]:
    support_paths = tuple(
        path.resolve()
        for path in sorted((report.evidence_dir / "support").glob("*.whl"))
    )
    declarations = driver.support_distribution_entries(support_paths)
    assert {
        entry["name"] for entry in declarations
    } == driver.SUPPORT_DISTRIBUTION_NAMES
    return declarations


def _sync_nvattest_receipt_with_ledger(
    receipt: dict[str, Any],
    *,
    nvattest: Mapping[str, Any],
    ledger_sha256: str,
) -> None:
    authority = nvattest["authority"]
    assert isinstance(authority, Mapping)
    authority_bytes = driver._canonical_nvattest_authority_bytes(authority)
    host = receipt["host"]
    assert isinstance(host, Mapping)
    target_key = host["authority_target_key"]
    authority_targets = authority["targets"]
    assert isinstance(authority_targets, Mapping)
    authority_target = authority_targets[target_key]
    assert isinstance(authority_target, Mapping)
    source = authority_target["source"]
    artifact = authority_target["artifact"]
    companion_manifest = authority_target["companion_manifest"]
    assert isinstance(source, Mapping)
    assert isinstance(artifact, Mapping)
    assert isinstance(companion_manifest, Mapping)

    receipt["challenge"] = nvattest["challenge"]
    receipt["ledger_sha256"] = ledger_sha256
    receipt["support_distributions"] = copy.deepcopy(nvattest["support_distributions"])
    receipt["installed_authority"]["sha256"] = nvattest["authority_sha256"]
    receipt["installed_authority"]["size_bytes"] = len(authority_bytes)
    receipt["nvattest"] = {
        "artifact": {
            "name": artifact["name"],
            "sha256": artifact["sha256"],
            "size_bytes": artifact["size_bytes"],
            "url": artifact["url"],
        },
        "companion_manifest": {
            "name": companion_manifest["name"],
            "sha256": companion_manifest["sha256"],
            "url": companion_manifest["url"],
        },
        "source": {
            "fork_commit": source["fork_commit"],
            "upstream_base": source["upstream_base"],
            "version": source["version"],
        },
        "target_key": target_key,
    }
    receipt["archive_fetch"] = {
        "sha256": artifact["sha256"],
        "size_bytes": artifact["size_bytes"],
        "url": artifact["url"],
    }
    receipt["manifest_fetch"]["sha256"] = companion_manifest["sha256"]
    receipt["manifest_fetch"]["url"] = companion_manifest["url"]
    receipt["companion_manifest"]["sha256"] = companion_manifest["sha256"]
    receipt["companion_manifest"]["target_key"] = target_key
    receipt["integrity"]["sidecar"]["artifact"] = dict(artifact)
    receipt["integrity"]["sidecar"]["target_key"] = target_key
    receipt["integrity"]["sidecar"]["version"] = source["version"]
    sidecar_bytes = checker.canonical_json_bytes(receipt["integrity"]["sidecar"])
    receipt["integrity"]["sidecar_sha256"] = hashlib.sha256(sidecar_bytes).hexdigest()
    receipt["integrity"]["sidecar_size_bytes"] = len(sidecar_bytes)


def _write_directory_sentinel(path: Path) -> None:
    marker = path / "inside" / "marker.txt"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(f"inside {path.name}", encoding="utf-8")
    outside = path.parent / f"outside-{path.name}.txt"
    outside.write_text(f"outside {path.name}", encoding="utf-8")


def _write_cleanup_preflight_sentinels(
    root: Path,
    dist_payload_root: Path | None,
    *,
    version: str,
) -> None:
    for path in (
        root / "build",
        root / "root-stale.egg-info",
        root / "packages" / "solstone-journal" / "solstone_journal.egg-info",
        root / "packages" / "solstone-journal-cuda" / "solstone_journal_cuda.egg-info",
        root
        / "packages"
        / "solstone-journal-models"
        / "solstone_journal_models.egg-info",
        root / "target" / "release-evidence" / version,
        root / "target" / "release-evidence" / f"{version}.staging",
        root / "target" / "release-transfer" / version,
        root / "target" / "release-transfer" / f".{version}.source.bundle",
        root / "target" / "release-transfer" / f".{version}.request-abc123",
        root / "target" / "release-zig-cache",
    ):
        _write_directory_sentinel(path)
    if dist_payload_root is None:
        return
    dist_payload_root.mkdir(parents=True, exist_ok=True)
    (dist_payload_root / "raw-build-output.whl").write_bytes(b"raw")
    _write_directory_sentinel(dist_payload_root / "raw-dir")
    reserved = dist_payload_root / driver.RESERVED_CANDIDATE_DIRNAME
    for name in (
        version,
        f"{version}.payload-staging",
        f"{version}.payload-staging.staging",
        f"{version}.payload-staging.quarantine",
    ):
        _write_directory_sentinel(reserved / name)


def _reserved_candidate_path(root: Path) -> Path:
    return root / "dist" / driver.RESERVED_CANDIDATE_DIRNAME


def _retained_current_paths(root: Path) -> tuple[Path, Path]:
    release_dir, _payload_staging, evidence_dir, _evidence_staging = _ready_paths(root)
    return release_dir, evidence_dir


def _write_retained_marker(path: Path, label: str) -> Path:
    marker = path / "sentinel" / f"{label}.txt"
    marker.parent.mkdir(parents=True)
    marker.write_text(f"retained {label}", encoding="utf-8")
    return marker


def _seed_retained_current_paths(
    root: Path, *, release: bool = True, evidence: bool = True
) -> tuple[Path | None, Path | None]:
    release_dir, evidence_dir = _retained_current_paths(root)
    release_marker = _write_retained_marker(release_dir, "payload") if release else None
    evidence_marker = (
        _write_retained_marker(evidence_dir, "evidence") if evidence else None
    )
    return release_marker, evidence_marker


def _tag_lookup_services(
    root: Path,
    state: driver.TagLookupState,
    *,
    commit: str | None = None,
    detail: str | None = None,
) -> driver.CandidateServices:
    services = _services(root)

    def git_tag_commit(_repo: Path, _version: str) -> driver.TagLookup:
        services.call_counts["git_tag_commit"] = (
            services.call_counts.get("git_tag_commit", 0) + 1
        )
        return driver.TagLookup(state=state, commit=commit, detail=detail)

    replaced = replace(services, git_tag_commit=git_tag_commit)
    object.__setattr__(replaced, "call_counts", services.call_counts)
    object.__setattr__(replaced, "reset_call_counts", services.reset_call_counts)
    return replaced


def _assert_retained_snapshots_unchanged(
    *,
    root: Path,
    release_dir: Path,
    evidence_dir: Path,
    before_release: tuple[TreeSnapshotEntry, ...],
    before_evidence: tuple[TreeSnapshotEntry, ...],
    markers: Sequence[Path | None],
) -> None:
    after_release = _structural_snapshot(release_dir)
    after_evidence = _structural_snapshot(evidence_dir)
    marker_status = ", ".join(
        f"{marker.relative_to(root).as_posix()} exists={marker.exists()}"
        for marker in markers
        if marker is not None
    )
    assert (after_release, after_evidence) == (before_release, before_evidence), (
        "retained snapshots changed; "
        f"{marker_status}; "
        f"before_release={before_release!r}; after_release={after_release!r}; "
        f"before_evidence={before_evidence!r}; after_evidence={after_evidence!r}"
    )


def _write_expected_local_dist(root: Path, *, include_models: bool) -> None:
    dist = root / "dist"
    dist.mkdir(parents=True, exist_ok=True)
    for name in driver._expected_local_dist_names(include_models=include_models):
        (dist / name).write_bytes(f"fixture package {name}\n".encode("utf-8"))


def _existing_expected_artifact_facts(
    root: Path, *, include_models: bool
) -> dict[str, tuple[str, int]]:
    dist = root / "dist"
    facts: dict[str, tuple[str, int]] = {}
    for name in driver._expected_local_dist_names(include_models=include_models):
        path = dist / name
        if not path.exists() or path.is_symlink():
            continue
        data = path.read_bytes()
        facts[name] = (hashlib.sha256(data).hexdigest(), path.stat().st_size)
    return facts


def _assert_reserved_parent_failure(
    exc: pytest.ExceptionInfo[driver.DriverError],
    *,
    operation: driver.DistPreflightOperation,
    actual: str,
    denied_access: bool = False,
) -> None:
    policy = driver.DIST_PREFLIGHT_POLICIES[operation]
    if denied_access:
        reserved_access = policy.reserved_access
        assert reserved_access is not None
        expected_error = reserved_access.access_error
    else:
        expected_error = policy.reserved_unsafe_error
    assert exc.value.failures[0].error == expected_error
    assert exc.value.failures[0].expected == driver._reserved_expected(policy)
    assert exc.value.failures[0].actual == actual
    assert exc.value.failures[0].repair == "bash scripts/release.sh --candidate"


def _assert_dist_preflight_failure(
    exc: pytest.ExceptionInfo[driver.DriverError],
    *,
    operation: driver.DistPreflightOperation,
    actual: str,
    denied_access: bool = False,
) -> None:
    policy = driver.DIST_PREFLIGHT_POLICIES[operation]
    assert exc.value.failures[0].error == (
        policy.dist_access_error if denied_access else policy.dist_unsafe_error
    )
    assert exc.value.failures[0].expected == driver._dist_expected(policy)
    assert exc.value.failures[0].actual == actual
    assert exc.value.failures[0].repair == "bash scripts/release.sh --candidate"


def _differing_payload_paths(
    first: object,
    second: object,
    path: tuple[str, ...] = (),
) -> set[tuple[str, ...]]:
    if isinstance(first, Mapping) and isinstance(second, Mapping):
        keys = set(first) | set(second)
        return {
            diff
            for key in keys
            for diff in _differing_payload_paths(
                first.get(key),
                second.get(key),
                (*path, str(key)),
            )
        }
    return set() if first == second else {path}


def _macos_release_dir_from_host_result(
    tmp_path: Path, host_result: BuildHostResult
) -> Path:
    release_dir = tmp_path / "release-dir"
    release_dir.mkdir()
    for wheel in host_result.macos_wheels:
        shutil.copy2(wheel, release_dir / wheel.name)
    return release_dir


def _native_record_payloads(host_result: BuildHostResult) -> list[dict[str, Any]]:
    return [
        json.loads(path.read_text(encoding="utf-8"))
        for path in host_result.native_records
    ]


def _macos_revalidation_inputs(
    tmp_path: Path,
) -> tuple[Path, list[dict[str, Any]]]:
    host_result = _write_macos_host_outputs(tmp_path / "host")
    return (
        _macos_release_dir_from_host_result(tmp_path, host_result),
        _native_record_payloads(host_result),
    )


def _record_by_role(records: Sequence[dict[str, Any]], role: str) -> dict[str, Any]:
    return next(record for record in records if record.get("role") == role)


@pytest.mark.release
def test_fake_all_host_candidate_and_recovery_are_deterministic(
    tmp_path: Path,
) -> None:
    first_root = _repo(tmp_path / "one")
    second_root = _repo(tmp_path / "two")

    first = driver.run_candidate(first_root, _env(), _services(first_root))
    second = driver.run_candidate(second_root, _env(), _services(second_root))

    assert first.heading == "candidate-proven"
    assert second.heading == "candidate-proven"
    assert not (
        first_root / "target" / "release-transfer" / checker._current_version()
    ).exists()
    assert not (
        first_root
        / "target"
        / "release-transfer"
        / f".{checker._current_version()}.source.bundle"
    ).exists()
    assert first.candidate_digest == second.candidate_digest
    first_ledger = json.loads(first.evidence_dir.joinpath("ledger.json").read_text())
    second_ledger = json.loads(second.evidence_dir.joinpath("ledger.json").read_text())
    assert _differing_payload_paths(first_ledger, second_ledger) == {
        ("nvattest", "challenge")
    }
    first_challenge = first_ledger["nvattest"]["challenge"]
    second_challenge = second_ledger["nvattest"]["challenge"]
    assert CHALLENGE_RE.fullmatch(first_challenge)
    assert CHALLENGE_RE.fullmatch(second_challenge)
    assert first_challenge != second_challenge
    assert sorted(path.name for path in first.release_dir.iterdir()) == sorted(
        path.name for path in second.release_dir.iterdir()
    )
    release_names = {path.name for path in first.release_dir.iterdir()}
    assert any(
        name.startswith("solstone_core-") and "manylinux2014_x86_64" in name
        for name in release_names
    )
    assert any(
        name.startswith("solstone_core-") and "manylinux2014_aarch64" in name
        for name in release_names
    )
    root_name, core_name, speakers_analyze_name = _macos_wheel_names()
    with zipfile.ZipFile(first.release_dir / root_name) as wheel:
        assert wheel.read(PARAKEET_HELPER_MEMBER) == MACOS_HELPER
    with zipfile.ZipFile(first.release_dir / core_name) as wheel:
        member = next(
            member
            for member in core_wheel_script_members(wheel)
            if Path(member.filename).name == "solstone-core"
        )
        assert wheel.read(member) == MACOS_CORE
    with zipfile.ZipFile(first.release_dir / speakers_analyze_name) as wheel:
        script_member = next(
            member
            for member in wheel.infolist()
            if member.filename.endswith(".data/scripts/solstone-core-speakers-analyze")
        )
        dylib_member = next(
            member
            for member in wheel.infolist()
            if member.filename.endswith(
                ".data/data/lib/solstone-core-speakers-analyze/"
                "libonnxruntime.1.25.0.dylib"
            )
        )
        assert wheel.read(script_member) == MACOS_SPEAKERS_ANALYZE
        assert wheel.read(dylib_member) == MACOS_ONNXRUNTIME

    recovered = _recover(first_root)
    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING
    assert recovered.bundle_digest == first.bundle_digest


def test_revalidate_macos_wheels_accepts_matching_unsigned_speakers_pin(
    tmp_path: Path,
) -> None:
    release_dir, records = _macos_revalidation_inputs(tmp_path)

    driver._revalidate_macos_wheels(
        release_dir,
        records,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=LOCK_SHA,
    )


def test_revalidate_macos_wheels_rejects_unsigned_speakers_dylib_pin_mismatch(
    tmp_path: Path,
) -> None:
    release_dir, records = _macos_revalidation_inputs(tmp_path)
    speakers = _record_by_role(records, "speakers-analyze")
    dylib_name = wheel_checker.SPEAKERS_ANALYZE_TARGETS[
        "macos-arm64"
    ].runtime_staged_name
    speakers["unsigned_members"][dylib_name] = "0" * 64

    with pytest.raises(driver.DriverError) as exc:
        driver._revalidate_macos_wheels(
            release_dir,
            records,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=LOCK_SHA,
        )

    assert any(
        failure.error
        == "macOS speakers-analyze unsigned ONNX Runtime hash does not match staged pin"
        for failure in exc.value.failures
    )


def test_revalidate_macos_wheels_rejects_unsigned_member_set_mismatch(
    tmp_path: Path,
) -> None:
    release_dir, records = _macos_revalidation_inputs(tmp_path)
    speakers = _record_by_role(records, "speakers-analyze")
    speakers["unsigned_members"].pop(wheel_checker.SPEAKERS_ANALYZE_SCRIPT_NAMES[0])

    with pytest.raises(driver.DriverError) as exc:
        driver._revalidate_macos_wheels(
            release_dir,
            records,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=LOCK_SHA,
        )

    assert any(
        failure.error == "macOS native record unsigned member set is wrong"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_recovery_uses_explicit_selector_and_preserves_retained_bytes(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)
    (root / "pyproject.toml").unlink()
    shutil.rmtree(root / "packages")

    recovered = driver.run_recover(
        root,
        version=report.version,
        source_commit=SOURCE_COMMIT,
    )

    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING
    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence


@pytest.mark.release
def test_recovery_ignores_current_release_metadata_drift(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))

    def fail_if_used(*_args: Any, **_kwargs: Any) -> Any:
        raise AssertionError("recovery read current release metadata")

    monkeypatch.setattr(driver, "expected_package_names", fail_if_used)
    monkeypatch.setattr(driver, "rust_artifact_targets", fail_if_used)
    monkeypatch.setattr(driver, "validate_release_dir", fail_if_used)
    # Guard checker globals reached via the recovery tombstone validator.
    monkeypatch.setattr(checker, "_current_version", fail_if_used)
    monkeypatch.setattr(checker, "expected_package_names", fail_if_used)
    monkeypatch.setattr(ledger, "rust_artifact_targets", fail_if_used)

    recovered = driver.run_recover(
        root,
        version=report.version,
        source_commit=SOURCE_COMMIT,
    )

    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING


@pytest.mark.release
def test_candidate_pair_promote_rejects_publication_prerequisite_in_staging(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    base_services = _services(root)

    def run_target_proofs(**kwargs: Any) -> Any:
        target_proofs = base_services.run_target_proofs(**kwargs)
        evidence_staging = target_proofs.install.parents[1]
        write_core_unsupported_tombstone_record(
            evidence_staging,
            checker._current_version(),
        )
        return target_proofs

    services = replace(base_services, run_target_proofs=run_target_proofs)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert any(
        failure.error == "release evidence inventory is not exact"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_candidate_final_recheck_rejects_publication_prerequisite_after_promotion(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)

    def hook(point: str) -> None:
        if point != "after-evidence-rename":
            return
        _ready_path, _payload_staging, evidence_dir, _evidence_staging = _ready_paths(
            root
        )
        write_core_unsupported_tombstone_record(
            evidence_dir,
            checker._current_version(),
        )

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert any(
        failure.error == "release evidence inventory is not exact"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_recovery_accepts_absent_publication_prerequisite(tmp_path: Path) -> None:
    root, _report = _real_candidate(tmp_path)

    recovered = _recover(root)
    payload = json.loads(driver.format_report(recovered))

    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING
    assert payload["retained_ledger_schema_version"] == 2
    assert payload["publication_prerequisite_inventory"] == []


@pytest.mark.release
def test_recovery_v2_retained_candidate_reports_current_heading(tmp_path: Path) -> None:
    root, _report = _real_candidate(tmp_path)

    recovered = _recover(root)
    payload = json.loads(driver.format_report(recovered))

    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING
    assert payload["retained_ledger_schema_version"] == 2


@pytest.mark.release
def test_recovery_accepts_valid_publication_prerequisite_and_reports_inventory_without_mutation(
    tmp_path: Path,
) -> None:
    root, report = _real_candidate(tmp_path)
    record = write_core_unsupported_tombstone_record(
        report.evidence_dir, report.version
    )
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)

    recovered = _recover(root)
    payload = json.loads(driver.format_report(recovered))

    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    assert payload["publication_prerequisite_inventory"] == [
        {
            "bytes": record.stat().st_size,
            "name": checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD,
            "sha256": driver.file_sha256_size(record)[0],
        }
    ]


@pytest.mark.parametrize("derive_v1", (False, True))
@pytest.mark.release
def test_recovery_tombstone_allowance_is_unchanged_for_registered_versions(
    tmp_path: Path,
    derive_v1: bool,
) -> None:
    root, report = _real_candidate(tmp_path)
    if derive_v1:
        _derive_pre_nvattest_v1_tree(report)
    write_core_unsupported_tombstone_record(
        report.evidence_dir,
        report.version,
    )

    recovered = _recover(root)
    payload = json.loads(driver.format_report(recovered))

    assert recovered.version == report.version
    assert payload["publication_prerequisite_inventory"]


@pytest.mark.release
def test_evidence_inventory_accepts_prerequisite_for_historical_retained_version(
    tmp_path: Path,
) -> None:
    _root, report = _real_candidate(tmp_path)
    write_core_unsupported_tombstone_record(
        report.evidence_dir,
        PRIOR_RETAINED_VERSION,
    )
    schema_version, schema = _retained_schema(report)

    failures = driver._validate_evidence_inventory(
        report.evidence_dir,
        schema_version=schema_version,
        schema=schema,
        publication_prerequisite_version=PRIOR_RETAINED_VERSION,
    )

    assert failures == []


@pytest.mark.release
def test_evidence_inventory_rejects_current_version_prerequisite_for_historical_retained_version(
    tmp_path: Path,
) -> None:
    _root, report = _real_candidate(tmp_path)
    write_core_unsupported_tombstone_record(
        report.evidence_dir,
        checker._current_version(),
    )
    schema_version, schema = _retained_schema(report)

    failures = driver._validate_evidence_inventory(
        report.evidence_dir,
        schema_version=schema_version,
        schema=schema,
        publication_prerequisite_version=PRIOR_RETAINED_VERSION,
    )

    assert any(
        failure.error
        == "core unsupported-platform tombstone prerequisite version is invalid"
        for failure in failures
    )


@pytest.mark.release
def test_recovery_resolves_v1_ledger_before_inventory_requires_nvattest(
    tmp_path: Path,
) -> None:
    root, report = _real_candidate(tmp_path)
    _derive_pre_nvattest_v1_tree(report)

    recovered = _recover(root)
    payload = json.loads(driver.format_report(recovered))

    assert recovered.heading == driver.RETAINED_PRE_NVATTEST_CANDIDATE_VALID_HEADING
    assert recovered.nvattest_sha256 == {}
    assert payload["retained_ledger_schema_version"] == 1
    assert payload["nvattest_sha256"] == {}
    assert "nvattest_inventory" not in payload
    assert "support_inventory" not in payload
    assert not (report.evidence_dir / "nvattest").exists()
    assert not (report.evidence_dir / "support").exists()


@pytest.mark.release
def test_report_missing_ledger_fails_with_named_error(tmp_path: Path) -> None:
    root, report = _real_candidate(tmp_path)
    _ledger_path(report).unlink()

    with pytest.raises(driver.DriverError) as exc:
        driver._report(
            heading=driver.RETAINED_CANDIDATE_VALID_HEADING,
            root=root,
            version=report.version,
            source_commit=SOURCE_COMMIT,
            expected_lock_sha256=LOCK_SHA,
            release_dir=report.release_dir,
            evidence_dir=report.evidence_dir,
            check_local_models_version=False,
            validate_current_release_metadata=False,
            allow_publication_prerequisite=True,
        )

    assert [(failure.error, failure.actual) for failure in exc.value.failures] == [
        ("retained ledger could not be read", "FileNotFoundError")
    ]


@pytest.mark.parametrize("entry", ("nvattest", "support"))
@pytest.mark.release
def test_pre_nvattest_v1_evidence_inventory_rejects_stray_nvattest_family(
    tmp_path: Path,
    entry: str,
) -> None:
    _root, report = _real_candidate(tmp_path)
    _derive_pre_nvattest_v1_tree(report)
    (report.evidence_dir / entry).mkdir()
    schema_version, schema = _retained_schema(report)

    failures = driver._validate_evidence_inventory(
        report.evidence_dir,
        schema_version=schema_version,
        schema=schema,
        publication_prerequisite_version=None,
    )

    assert failures
    assert (failures[0].error, failures[0].expected) == (
        "release evidence inventory is not exact",
        "schema_version 1: ledger.json, proofs",
    )


@pytest.mark.release
def test_pre_nvattest_v1_consumers_fail_loudly_by_version(
    tmp_path: Path,
) -> None:
    _root, report = _real_candidate(tmp_path)
    _derive_pre_nvattest_v1_tree(report)
    ledger_payload = _read_json(_ledger_path(report))
    schema_version, schema = _retained_schema(report)
    ledger_sha256 = driver.file_sha256_size(_ledger_path(report))[0]

    def capture(call: Callable[[], object]) -> tuple[str, object]:
        try:
            result = call()
        except driver.DriverError as exc:
            failures = [
                {
                    "actual": failure.actual,
                    "error": failure.error,
                    "expected": failure.expected,
                }
                for failure in exc.failures
            ]
            return ("DriverError", failures)
        except Exception as exc:  # noqa: BLE001 - this test records exact diagnostics.
            return (type(exc).__name__, str(exc))
        return ("return", result)

    expected_evidence_names = {"ledger.json"} | {
        f"proofs/{target}.json" for target in driver.PROOF_TARGETS
    }
    expected_failure = [
        {
            "actual": "schema_version 1",
            "error": "retained ledger schema does not declare nvattest",
            "expected": "schema with nvattest binding",
        }
    ]

    observed = {
        "evidence_report_inventory": capture(
            lambda: {
                entry["name"]
                for entry in driver._evidence_report_inventory(
                    report.evidence_dir,
                    schema=schema,
                )
            }
        ),
        "nvattest_report_inventory": capture(
            lambda: driver._nvattest_report_inventory(
                report.evidence_dir,
                schema=schema,
            )
        ),
        "support_report_inventory": capture(
            lambda: driver._support_report_inventory(
                report.evidence_dir,
                schema=schema,
            )
        ),
        "retained_support_binding": capture(
            lambda: driver._retained_support_binding_failures(
                evidence_dir=report.evidence_dir,
                ledger=ledger_payload,
            )
        ),
        "retained_authority_binding": capture(
            lambda: driver._retained_authority_binding(
                release_dir=report.release_dir,
                ledger=ledger_payload,
            )
        ),
        "retained_nvattest_binding": capture(
            lambda: driver._validate_retained_nvattest_binding(
                evidence_dir=report.evidence_dir,
                ledger=ledger_payload,
                release_dir=report.release_dir,
                digest=report.candidate_digest,
                ledger_sha256=ledger_sha256,
                version=report.version,
            )
        ),
    }

    assert observed == {
        "evidence_report_inventory": ("return", expected_evidence_names),
        "nvattest_report_inventory": ("return", {}),
        "support_report_inventory": ("return", []),
        "retained_support_binding": ("DriverError", expected_failure),
        "retained_authority_binding": ("DriverError", expected_failure),
        "retained_nvattest_binding": ("DriverError", expected_failure),
    }


@pytest.mark.parametrize(
    ("record_kind", "expected_error"),
    [
        (
            "symlink",
            "core unsupported-platform tombstone prerequisite is a symlink",
        ),
        (
            "directory",
            "core unsupported-platform tombstone prerequisite is not a regular file",
        ),
    ],
)
@pytest.mark.release
def test_recovery_rejects_publication_prerequisite_metadata_mutations_without_reading(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    record_kind: str,
    expected_error: str,
) -> None:
    root, report = _real_candidate(tmp_path)
    record = report.evidence_dir / checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD
    outside = tmp_path / "outside-record.json"
    write_core_unsupported_tombstone_record(tmp_path, report.version)
    (tmp_path / checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD).rename(outside)
    if record_kind == "symlink":
        record.symlink_to(outside)
    elif record_kind == "directory":
        record.mkdir()
    read_calls: list[Path] = []
    original_read_text = Path.read_text

    def read_text(path: Path, *args: Any, **kwargs: Any) -> str:
        if path in {record, outside}:
            read_calls.append(path)
        return original_read_text(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", read_text)

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert read_calls == []
    assert any(failure.error == expected_error for failure in exc.value.failures)


@pytest.mark.parametrize(
    ("case", "expected_error"),
    [
        (
            "unreadable",
            "core unsupported-platform tombstone prerequisite could not be read",
        ),
        (
            "malformed-json",
            "core unsupported-platform tombstone prerequisite is not valid JSON",
        ),
        (
            "non-mapping",
            "core unsupported-platform tombstone prerequisite is invalid",
        ),
        (
            "wrong-version",
            "core unsupported-platform tombstone prerequisite version is invalid",
        ),
    ],
)
@pytest.mark.release
def test_recovery_rejects_publication_prerequisite_read_parse_and_schema_failures_without_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    case: str,
    expected_error: str,
) -> None:
    root, report = _real_candidate(tmp_path)
    record = write_core_unsupported_tombstone_record(
        report.evidence_dir,
        report.version,
        mutation=None if case == "unreadable" else case,
    )
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)
    if case == "unreadable":
        original_read_text = Path.read_text

        def read_text(path: Path, *args: Any, **kwargs: Any) -> str:
            if path == record:
                raise OSError("injected read failure")
            return original_read_text(path, *args, **kwargs)

        monkeypatch.setattr(Path, "read_text", read_text)

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    assert any(failure.error == expected_error for failure in exc.value.failures)


@pytest.mark.parametrize(
    ("mutation", "expected_error"),
    [
        ("fourth-top-level", "release evidence inventory is not exact"),
        ("wrong-named-third", "release evidence inventory is not exact"),
        ("nonregular-ledger", "retained ledger could not be read for selector"),
        ("symlinked-proofs", "release proofs entry is not an owned directory"),
        ("wrong-proof-name-set", "release proof inventory is not exact"),
    ],
)
@pytest.mark.release
def test_recovery_mode_does_not_weaken_rest_of_evidence_gate(
    tmp_path: Path,
    mutation: str,
    expected_error: str,
) -> None:
    root, report = _real_candidate(tmp_path)
    if mutation != "wrong-named-third":
        write_core_unsupported_tombstone_record(report.evidence_dir, report.version)
    if mutation == "fourth-top-level":
        (report.evidence_dir / "extra.json").write_text("extra", encoding="utf-8")
    elif mutation == "wrong-named-third":
        (report.evidence_dir / "wrong-name.json").write_text(
            "wrong",
            encoding="utf-8",
        )
    elif mutation == "nonregular-ledger":
        (report.evidence_dir / "ledger.json").unlink()
        (report.evidence_dir / "ledger.json").mkdir()
    elif mutation == "symlinked-proofs":
        outside = tmp_path / "outside-proofs"
        outside.mkdir()
        shutil.rmtree(report.evidence_dir / "proofs")
        (report.evidence_dir / "proofs").symlink_to(outside, target_is_directory=True)
    elif mutation == "wrong-proof-name-set":
        proof = report.evidence_dir / "proofs" / "macos-arm64.json"
        proof.rename(report.evidence_dir / "proofs" / "macos-arm64-wrong.json")

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(failure.error == expected_error for failure in exc.value.failures)


@pytest.mark.release
def test_recovery_rejects_publication_prerequisite_in_payload_directory(
    tmp_path: Path,
) -> None:
    root, report = _real_candidate(tmp_path)
    write_core_unsupported_tombstone_record(report.release_dir, report.version)

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(
        failure.error == "release payload inventory is not exact"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_recovery_identity_is_invariant_across_absent_and_present_prerequisite(
    tmp_path: Path,
) -> None:
    root, report = _real_candidate(tmp_path)
    absent = _recover(root)
    absent_payload = json.loads(driver.format_report(absent))
    write_core_unsupported_tombstone_record(report.evidence_dir, report.version)
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)

    present = _recover(root)
    present_payload = json.loads(driver.format_report(present))

    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    for field in (
        "candidate_digest",
        "ledger_sha256",
        "proof_sha256",
        "nvattest_sha256",
        "bundle_digest",
        "payload_files",
    ):
        assert getattr(absent, field) == getattr(present, field)
    for key in (
        "payload_inventory",
        "evidence_inventory",
        "proof_inventory",
        "nvattest_inventory",
        "support_inventory",
    ):
        assert absent_payload[key] == present_payload[key]
    assert absent_payload.pop("publication_prerequisite_inventory") == []
    prerequisite_inventory = present_payload.pop("publication_prerequisite_inventory")
    assert prerequisite_inventory
    assert absent_payload == present_payload


@pytest.mark.release
def test_transparency_snapshot_recovery_accepts_prerequisite_without_staging_it(
    tmp_path: Path,
) -> None:
    root, report = _real_candidate(tmp_path)
    write_core_unsupported_tombstone_record(report.evidence_dir, report.version)
    snapshot_root = tmp_path / "snapshot"

    snapshot_candidate(
        source_root=root,
        snapshot_root=snapshot_root,
        version=report.version,
    )
    recovered = driver.run_recover(
        snapshot_root,
        version=report.version,
        source_commit=SOURCE_COMMIT,
    )
    parts = collect_candidate_parts(recovered)

    assert (
        snapshot_root
        / "target"
        / "release-evidence"
        / report.version
        / checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD
    ).is_file()
    assert checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD not in parts.version_files
    assert checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD not in parts.artifact_files
    assert all(
        item.name != checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD
        for item in (*parts.artifacts, *parts.manifests, *parts.proofs)
    )


@pytest.mark.release
def test_fresh_cleanup_preserves_other_retained_versions_and_recovery(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)
    raw_dist_file = root / "dist" / "raw-build-output.whl"
    raw_dist_file.write_bytes(b"raw")

    driver._default_clean_outputs(root, "9.9.9")

    assert not raw_dist_file.exists()
    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    recovered = driver.run_recover(
        root,
        version=report.version,
        source_commit=SOURCE_COMMIT,
    )
    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING


@pytest.mark.release
def test_candidate_refuses_published_retained_payload_and_evidence_before_cleanup(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    release_dir, evidence_dir = _retained_current_paths(root)
    markers = _seed_retained_current_paths(root)
    before_release = _structural_snapshot(release_dir)
    before_evidence = _structural_snapshot(evidence_dir)
    services = _tag_lookup_services(root, "present", commit=SOURCE_COMMIT)
    caught: driver.DriverError | None = None

    try:
        driver.run_candidate(root, _env(), services)
    except driver.DriverError as exc:
        caught = exc

    _assert_retained_snapshots_unchanged(
        root=root,
        release_dir=release_dir,
        evidence_dir=evidence_dir,
        before_release=before_release,
        before_evidence=before_evidence,
        markers=markers,
    )
    assert caught is not None
    failure = caught.failures[0]
    assert failure.error == "published retained release evidence would be discarded"
    assert failure.expected == (
        "no retained release-candidate payload/evidence for published tag, or "
        "RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG=<version>+<tag>"
    )
    assert failure.actual == (
        f"version {version}; tag v{version} -> {SOURCE_COMMIT}; "
        "present retained paths: "
        f"dist/release-candidate/{version}, target/release-evidence/{version}"
    )
    assert failure.repair == (
        "set "
        f"{driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}={version}+v{version} "
        f"to discard retained payload/evidence for published tag v{version}"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_allows_unpublished_retained_evidence_with_soft_authorization(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root)
    services = _services(root)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    failure = exc.value.failures[0]
    assert failure.error == "retained release evidence would be discarded"
    assert failure.actual == (
        f"working-tree version {version}; colliding retained paths: "
        f"dist/release-candidate/{version}, target/release-evidence/{version}; "
        "absent retained paths: <none>"
    )
    _assert_no_post_guard_service_calls(services)
    _reset_service_call_counts(services)
    env = _env()
    env[driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV] = version

    report = driver.run_candidate(root, env, services)

    assert report.heading == "candidate-proven"
    assert services.call_counts["clean_outputs"] == 1


@pytest.mark.release
def test_candidate_refuses_published_retained_payload_only(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root, evidence=False)
    services = _tag_lookup_services(root, "present", commit=SOURCE_COMMIT)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    failure = exc.value.failures[0]
    assert failure.error == "published retained release evidence would be discarded"
    assert failure.actual == (
        f"version {version}; tag v{version} -> {SOURCE_COMMIT}; "
        f"present retained paths: dist/release-candidate/{version}"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_refuses_published_retained_evidence_only(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root, release=False)
    services = _tag_lookup_services(root, "present", commit=SOURCE_COMMIT)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    failure = exc.value.failures[0]
    assert failure.error == "published retained release evidence would be discarded"
    assert failure.actual == (
        f"version {version}; tag v{version} -> {SOURCE_COMMIT}; "
        f"present retained paths: target/release-evidence/{version}"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_refuses_discard_authorization_for_other_version(
    tmp_path: Path,
) -> None:
    version = checker._current_version()
    cases = (
        (
            "soft",
            driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV,
            PRIOR_RETAINED_VERSION,
            None,
            (
                f"set {driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}={version} "
                f"or unset {driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}"
            ),
        ),
        (
            "hard",
            driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV,
            f"{PRIOR_RETAINED_VERSION}+v{PRIOR_RETAINED_VERSION}",
            SOURCE_COMMIT,
            (
                f"set {driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}="
                f"{version}+v{version} "
                f"or unset {driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}"
            ),
        ),
    )
    for name, variable, value, commit, repair in cases:
        root = _repo(tmp_path / name)
        _seed_retained_current_paths(root)
        services = (
            _tag_lookup_services(root, "present", commit=commit)
            if commit is not None
            else _services(root)
        )
        env = _env()
        env[variable] = value

        with pytest.raises(driver.DriverError) as exc:
            driver.run_candidate(root, env, services)

        failure = exc.value.failures[0]
        assert (
            failure.error
            == "release candidate discard authorization names a different version"
        )
        assert failure.expected == (
            f"{variable}=<version> matching working-tree version {version}"
        )
        assert failure.actual == (
            f"{variable}={value}; authorization version {PRIOR_RETAINED_VERSION}; "
            f"working-tree version {version}"
        )
        assert failure.repair == repair
        _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_soft_authorization_does_not_clear_published_tag_tier(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root)
    services = _tag_lookup_services(root, "present", commit=SOURCE_COMMIT)
    env = _env()
    env[driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV] = version

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, env, services)

    assert exc.value.failures[0].error == (
        "published retained release evidence would be discarded"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_hard_authorization_satisfies_unpublished_soft_tier(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root)
    services = _services(root)
    env = _env()
    env[driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV] = f"{version}+v{version}"

    report = driver.run_candidate(root, env, services)

    assert report.heading == "candidate-proven"
    assert services.call_counts["clean_outputs"] == 1


@pytest.mark.release
def test_candidate_refuses_undeterminable_retained_path_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    release_dir, _evidence_dir = _retained_current_paths(root)
    release_dir.parent.mkdir(parents=True)
    original_lstat = Path.lstat

    def lstat(path: Path) -> os.stat_result:
        if path == release_dir:
            raise PermissionError("denied")
        return original_lstat(path)

    monkeypatch.setattr(Path, "lstat", lstat)
    services = _services(root)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    failure = exc.value.failures[0]
    assert failure.error == "retained release evidence state is undeterminable"
    assert failure.actual == (
        f"retained path check could not inspect "
        f"dist/release-candidate/{version}: PermissionError"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_refuses_undeterminable_tag_lookup(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    _seed_retained_current_paths(root)
    services = _tag_lookup_services(
        root,
        "undeterminable",
        detail="git rev-parse exit 128",
    )
    env = _env()
    env[driver.RELEASE_CANDIDATE_DISCARD_RETAINED_ENV] = version
    env[driver.RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV] = f"{version}+v{version}"

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, env, services)

    failure = exc.value.failures[0]
    assert failure.error == "retained release evidence state is undeterminable"
    assert failure.actual == (
        f"tag lookup for v{version} was undeterminable: git rev-parse exit 128"
    )
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_candidate_default_fixture_refuses_retained_path_without_authorization(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    _seed_retained_current_paths(root, evidence=False)
    services = _services(root)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert exc.value.failures[0].error == "retained release evidence would be discarded"
    assert services.call_counts["git_tag_commit"] == 1
    _assert_no_post_guard_service_calls(services)


@pytest.mark.release
def test_recovery_rejects_absent_or_mutated_selector(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    driver.run_candidate(root, _env(), _services(root))

    with pytest.raises(driver.DriverError) as exc:
        driver.run_recover(root, version="", source_commit=SOURCE_COMMIT)
    assert exc.value.failures[0].error == "retained release version selector is missing"

    with pytest.raises(driver.DriverError) as exc:
        driver.run_recover(root, version="../0.9.0", source_commit=SOURCE_COMMIT)
    assert exc.value.failures[0].error == "retained release version selector is unsafe"

    with pytest.raises(driver.DriverError) as exc:
        driver.run_recover(
            root,
            version=checker._current_version(),
            source_commit="b" * 40,
        )
    assert (
        exc.value.failures[0].error
        == "retained ledger source commit does not match selector"
    )

    with pytest.raises(driver.DriverError) as exc:
        driver.run_recover(root, version="0.0.0", source_commit=SOURCE_COMMIT)
    assert (
        exc.value.failures[0].error == "retained ledger could not be read for selector"
    )


@pytest.mark.release
def test_recovery_rejects_garbage_retained_advisory_identity(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger_path = report.evidence_dir / "ledger.json"
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    payload["policy_run"]["db_commit"] = "not-hex"
    ledger_path.write_bytes(checker.canonical_json_bytes(payload))

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(
        failure.error.endswith(".db_commit is invalid")
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_recovery_rejects_impossible_retained_policy_timestamp(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger_path = report.evidence_dir / "ledger.json"
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    payload["policy_run"]["db_commit_timestamp"] = "2026-99-19T12:00:00Z"
    ledger_path.write_bytes(checker.canonical_json_bytes(payload))

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(
        failure.error == "retained ledger db_commit_timestamp is invalid"
        for failure in exc.value.failures
    )


def test_recovery_has_no_service_surface() -> None:
    parameters = set(inspect.signature(driver.run_recover).parameters)

    assert parameters == {"root", "version", "source_commit"}


@pytest.mark.release
def test_machine_report_is_canonical_sorted_and_not_publication_authorization(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    candidate = driver.run_candidate(root, _env(), _services(root))
    retained = _recover(root)

    for report in (candidate, retained):
        text = driver.format_report(report)
        payload = json.loads(text)
        assert text.encode("utf-8") == checker.canonical_json_bytes(payload)
        assert payload["verdict"] == report.heading
        assert (
            payload["publication_authorization"]
            == "local candidate evidence only; not publication authorization"
        )
        payload_names = [item["name"] for item in payload["payload_inventory"]]
        evidence_names = [item["name"] for item in payload["evidence_inventory"]]
        assert payload_names == sorted(payload_names)
        assert evidence_names == sorted(evidence_names)
        assert payload["publication_prerequisite_inventory"] == []
        assert payload["candidate_digest"] == driver.candidate_digest(
            report.release_dir
        )
        assert (
            payload["ledger_sha256"]
            == driver.file_sha256_size(report.evidence_dir / "ledger.json")[0]
        )
        for target, entry in payload["proof_inventory"].items():
            assert entry["sha256"] == payload["proof_sha256"][target]
        for target, entry in payload["nvattest_inventory"].items():
            assert entry["sha256"] == payload["nvattest_sha256"][target]


@pytest.mark.release
def test_candidate_cleanup_receives_release_zig_cache_root(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    services = _services(root)
    version = checker._current_version()
    cache_root = root / "target" / "release-zig-cache"
    (cache_root / "zig-global").mkdir(parents=True)
    (cache_root / "zig-global" / "marker").write_text("stale", encoding="utf-8")
    cleanup_calls: list[tuple[Path, ...]] = []

    def cleanup(paths: Sequence[Path]) -> None:
        cleanup_calls.append(tuple(paths))
        services.cleanup_transients(paths)

    driver.run_candidate(
        root,
        _env(),
        replace(services, cleanup_transients=cleanup),
    )

    assert cleanup_calls == [
        (
            root / "target" / "release-transfer" / version,
            root / "target" / "release-transfer" / f".{version}.source.bundle",
            cache_root,
        )
    ]
    assert not cache_root.exists()


@pytest.mark.release
def test_candidate_source_bundle_does_not_preexist_build_host_output(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    services = _services(root)
    original = services.build_host
    checked_output_dirs: list[Path] = []

    def build_host(
        source_bundle: SourceBundle, commit: str, output_dir: Path
    ) -> BuildHostResult:
        release_build_host._validate_fresh_directory_path(output_dir, label="output")
        checked_output_dirs.append(output_dir)
        return original(source_bundle, commit, output_dir)

    driver.run_candidate(root, _env(), replace(services, build_host=build_host))

    assert checked_output_dirs == [
        root / "target" / "release-transfer" / checker._current_version()
    ]


def test_dry_run_linux_validates_static_plan_without_files_or_services(
    tmp_path: Path,
) -> None:
    before = sorted(tmp_path.rglob("*"))
    output = driver.run_dry_run_linux(tmp_path, _env())

    assert sorted(tmp_path.rglob("*")) == before
    assert "validated" in output
    assert "candidate-proven" not in output
    assert "clean-source claim" in output


@pytest.mark.parametrize(
    "mutation",
    ["artifact", "model", "tool", "build-arg", "lockout"],
)
def test_dry_run_linux_rejects_bad_plan_cases(
    tmp_path: Path,
    mutation: str,
) -> None:
    plan = driver.default_dry_run_plan(_env())
    if mutation == "artifact":
        plan = replace(plan, artifacts=plan.artifacts[:-1])
    elif mutation == "model":
        plan = replace(plan, models_decision="publish")
    elif mutation == "tool":
        tools = {lane: dict(values) for lane, values in plan.tool_evidence.items()}
        tools["source"]["rustc"] = "rustc 0.0.0"
        plan = replace(plan, tool_evidence=tools)
    elif mutation == "build-arg":
        args = dict(plan.linux_maturin_args)
        args["x86_64-unknown-linux-musl"] = args["x86_64-unknown-linux-musl"].replace(
            "--locked ", ""
        )
        plan = replace(plan, linux_maturin_args=args)
    elif mutation == "lockout":
        lockout = dict(plan.publication_lockout)
        lockout["make release"] = False
        plan = replace(plan, publication_lockout=lockout)

    with pytest.raises(driver.DriverError):
        driver.run_dry_run_linux(tmp_path, _env(), plan=plan)


def test_main_prints_failure_records_from_build_host_errors(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    failure = checker.Failure(
        error="distinct build-host failure",
        expected="distinct expected value",
        actual="distinct actual value",
        repair="distinct repair command",
    )

    def run_candidate(*_args: object, **_kwargs: object) -> driver.CandidateReport:
        raise release_build_host.BuildHostError([failure])

    monkeypatch.setattr(driver, "run_candidate", run_candidate)

    assert driver.main(["candidate"], _env()) == 1
    captured = capsys.readouterr()

    assert captured.out == ""
    assert "ERROR: distinct build-host failure" in captured.err
    assert "expected: distinct expected value" in captured.err
    assert "actual: distinct actual value" in captured.err
    assert "repair command: distinct repair command" in captured.err
    assert "actual: BuildHostError" not in captured.err


def test_main_preserves_generic_fallback_for_plain_exceptions(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def run_candidate(*_args: object, **_kwargs: object) -> driver.CandidateReport:
        raise RuntimeError("boom")

    monkeypatch.setattr(driver, "run_candidate", run_candidate)

    assert driver.main(["candidate"], _env()) == 1
    captured = capsys.readouterr()

    assert captured.out == ""
    assert "ERROR: release candidate driver failed" in captured.err
    assert "actual: RuntimeError" in captured.err


def test_main_uses_generic_fallback_for_invalid_failure_records(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    class EmptyFailuresError(RuntimeError):
        failures: tuple[object, ...] = ()

    class FailureShapeError(RuntimeError):
        def __init__(self, failures: object) -> None:
            self.failures = failures
            super().__init__("boom")

    cases = (
        RuntimeError("boom"),
        EmptyFailuresError("boom"),
        FailureShapeError("boom"),
        FailureShapeError(b"boom"),
        FailureShapeError(("not a failure",)),
    )
    for exc in cases:

        def run_candidate(
            *_args: object,
            _exc: BaseException = exc,
            **_kwargs: object,
        ) -> driver.CandidateReport:
            raise _exc

        monkeypatch.setattr(driver, "run_candidate", run_candidate)

        assert driver.main(["candidate"], _env()) == 1
        captured = capsys.readouterr()
        assert captured.out == ""
        assert "ERROR: release candidate driver failed" in captured.err
        assert f"actual: {type(exc).__name__}" in captured.err


@pytest.mark.parametrize(
    ("point", "exc_factory"),
    [
        ("after-payload-rename", RuntimeError),
        ("between-renames", RuntimeError),
        ("after-evidence-rename", RuntimeError),
        ("after-payload-rename", KeyboardInterrupt),
        ("between-renames", SystemExit),
    ],
)
@pytest.mark.release
def test_candidate_transaction_rolls_back_payload_and_evidence_at_each_rename_point(
    tmp_path: Path,
    point: str,
    exc_factory: type[BaseException],
) -> None:
    root = _repo(tmp_path)
    foreign_payload = root / "dist" / "release-candidate" / "foreign"
    foreign_evidence = root / "target" / "release-evidence" / "foreign"

    def hook(actual_point: str) -> None:
        if actual_point == point:
            foreign_payload.mkdir(parents=True)
            foreign_evidence.mkdir(parents=True)
            (foreign_payload / "keep").write_text("payload", encoding="utf-8")
            (foreign_evidence / "keep").write_text("evidence", encoding="utf-8")
            raise exc_factory()

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert (foreign_payload / "keep").read_text(encoding="utf-8") == "payload"
    assert (foreign_evidence / "keep").read_text(encoding="utf-8") == "evidence"
    assert any(
        failure.error == "release candidate finalization transaction failed"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_candidate_transaction_aggregates_cleanup_errors(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    outside = tmp_path / "foreign"
    outside.mkdir()
    (outside / "keep").write_text("keep", encoding="utf-8")

    def hook(point: str) -> None:
        if point == "after-payload-rename":
            ready_path, _payload_staging, _evidence_dir, _evidence_staging = (
                _ready_paths(root)
            )
            shutil.rmtree(ready_path)
            ready_path.symlink_to(outside, target_is_directory=True)
            raise RuntimeError()

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert (outside / "keep").read_text(encoding="utf-8") == "keep"
    assert any(
        failure.error == "release candidate finalization transaction failed"
        for failure in exc.value.failures
    )
    assert any("symlink residue" in failure.error for failure in exc.value.failures)


@pytest.mark.parametrize("mutation", ["nested", "extra", "missing", "symlink"])
@pytest.mark.release
def test_candidate_final_recheck_rejects_payload_inventory_mutations(
    tmp_path: Path,
    mutation: str,
) -> None:
    root = _repo(tmp_path)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "payload.whl").write_text("outside", encoding="utf-8")

    def hook(point: str) -> None:
        if point != "after-evidence-rename":
            return
        ready_path, _payload_staging, _evidence_dir, _evidence_staging = _ready_paths(
            root
        )
        first_file = next(path for path in ready_path.iterdir() if path.is_file())
        if mutation == "nested":
            nested = ready_path / "nested"
            nested.mkdir()
            (nested / "extra.txt").write_text("extra", encoding="utf-8")
        elif mutation == "extra":
            (ready_path / "extra.whl").write_text("extra", encoding="utf-8")
        elif mutation == "missing":
            first_file.unlink()
        elif mutation == "symlink":
            first_file.unlink()
            first_file.symlink_to(outside / "payload.whl")

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert any("payload" in failure.error for failure in exc.value.failures)


@pytest.mark.parametrize("mutation", ["extra", "temp", "directory", "proof-symlink"])
@pytest.mark.release
def test_candidate_final_recheck_rejects_evidence_inventory_mutations(
    tmp_path: Path,
    mutation: str,
) -> None:
    root = _repo(tmp_path)
    outside = tmp_path / "outside-proof.json"
    outside.write_text("{}", encoding="utf-8")

    def hook(point: str) -> None:
        if point != "after-evidence-rename":
            return
        _ready_path, _payload_staging, evidence_dir, _evidence_staging = _ready_paths(
            root
        )
        if mutation == "extra":
            (evidence_dir / "extra.json").write_text("extra", encoding="utf-8")
        elif mutation == "temp":
            (evidence_dir / ".ledger.json.tmp").write_text("temp", encoding="utf-8")
        elif mutation == "directory":
            (evidence_dir / "extra-dir").mkdir()
        elif mutation == "proof-symlink":
            proof = evidence_dir / "proofs" / "macos-arm64.json"
            proof.unlink()
            proof.symlink_to(outside)

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert any(
        "evidence" in failure.error or "proof" in failure.error
        for failure in exc.value.failures
    )


@pytest.mark.parametrize(
    "mutation",
    [
        "kind",
        "product",
        "version",
        "source_commit",
        "core_lock_sha256",
        "rust_targets",
        "proofs",
        "redaction",
        "policy_result",
        "advisory_source_id",
        "native_summary",
    ],
)
@pytest.mark.release
def test_candidate_final_recheck_rejects_deep_ledger_binding_mutations(
    tmp_path: Path,
    mutation: str,
) -> None:
    root = _repo(tmp_path)

    def hook(point: str) -> None:
        if point != "after-evidence-rename":
            return
        _ready_path, _payload_staging, evidence_dir, _evidence_staging = _ready_paths(
            root
        )
        ledger_path = evidence_dir / "ledger.json"
        payload = json.loads(ledger_path.read_text(encoding="utf-8"))
        if mutation == "kind":
            payload["kind"] = "forged-ledger"
        elif mutation == "product":
            payload["product"] = "other"
        elif mutation == "version":
            payload["version"] = "0.0.0"
        elif mutation == "source_commit":
            payload["source_commit"] = "b" * 40
        elif mutation == "core_lock_sha256":
            payload["core_lock_sha256"] = "0" * 64
        elif mutation == "rust_targets":
            payload["rust_targets"] = []
        elif mutation == "proofs":
            payload["proofs"]["expected_targets"] = ["macos-arm64"]
        elif mutation == "redaction":
            payload["redaction"]["validator"] = "none"
        elif mutation == "policy_result":
            payload["policy_run"]["result"] = "fail"
        elif mutation == "advisory_source_id":
            payload["policy_run"]["advisory_source_id"] = ""
        elif mutation == "native_summary":
            payload["native_summary"]["macos_root_helper"]["wheel"]["sha256"] = "0" * 64
        ledger_path.write_bytes(checker.canonical_json_bytes(payload))

    services = replace(_services(root), transaction_hook=hook)

    with pytest.raises(driver.DriverError):
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)


@pytest.mark.release
def test_candidate_final_recheck_rejects_clean_status_drift_and_rolls_back(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    calls = 0

    def git_status(_repo: Path) -> str:
        nonlocal calls
        calls += 1
        return " M late-change" if calls == 3 else ""

    services = replace(_services(root), git_status=git_status)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert exc.value.failures[0].error == "release source tree is not clean"


@pytest.mark.release
def test_candidate_final_recheck_rejects_core_lock_drift_and_rolls_back(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    calls = 0

    def core_lock_sha256(_repo: Path) -> str:
        nonlocal calls
        calls += 1
        return "0" * 64 if calls == 4 else LOCK_SHA

    services = replace(_services(root), core_lock_sha256=core_lock_sha256)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert exc.value.failures[0].error == "core lock hash changed before finalization"


@pytest.mark.release
def test_recovery_rejects_swapped_replayed_or_mutated_proofs(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    proof = report.evidence_dir / "proofs" / "macos-arm64.json"
    payload = json.loads(proof.read_text(encoding="utf-8"))
    payload["candidate_digest"] = "0" * 64
    proof.write_text(json.dumps(payload), encoding="utf-8")

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(
        failure.error
        == "install proof candidate_digest is not bound to retained candidate"
        for failure in exc.value.failures
    )


@pytest.mark.parametrize(
    "argv",
    [
        ("bash", "scripts/release.sh"),
        ("bash", "scripts/release.sh", "--test"),
        ("make", "release"),
        ("make", "release-test"),
    ],
)
@pytest.mark.release
def test_publication_entrypoints_fail_closed_before_external_seams(
    tmp_path: Path, argv: Sequence[str]
) -> None:
    sentinel_dir = tmp_path / "sentinels"
    sentinel_dir.mkdir()
    log = tmp_path / "sentinel.log"
    for name in (
        "ssh",
        "rsync",
        "twine",
        "uvx",
        "gh",
        "git",
        "curl",
        "uv",
        "cargo",
        "codesign",
        "xcrun",
    ):
        path = sentinel_dir / name
        path.write_text(
            f'#!/bin/sh\necho {name} "$@" >> {log}\nexit 99\n',
            encoding="utf-8",
        )
        path.chmod(0o755)
    env = {
        "PATH": f"{sentinel_dir}{os.pathsep}{os.environ['PATH']}",
        "HOME": str(tmp_path / "home"),
    }

    result = subprocess.run(
        list(argv),
        cwd=Path(__file__).resolve().parent.parent,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "make publish-release" in result.stderr
    assert "scripts/release_publish.py" in result.stderr
    assert "release-publisher" not in result.stderr
    assert not log.exists() or log.read_text(encoding="utf-8") == ""


@pytest.mark.release
def test_deleted_all_hosts_mode_is_unknown_without_external_seams(
    tmp_path: Path,
) -> None:
    sentinel_dir = tmp_path / "sentinels"
    sentinel_dir.mkdir()
    log = tmp_path / "sentinel.log"
    for name in ("git", "ssh", "uv", "uvx", "cargo"):
        path = sentinel_dir / name
        path.write_text(
            f'#!/bin/sh\necho {name} "$@" >> {log}\nexit 99\n',
            encoding="utf-8",
        )
        path.chmod(0o755)
    result = subprocess.run(
        ["bash", "scripts/release.sh", "--dry-run-all-hosts"],
        cwd=Path(__file__).resolve().parent.parent,
        env={
            "PATH": f"{sentinel_dir}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(tmp_path / "home"),
        },
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 2
    assert "unknown argument: --dry-run-all-hosts" in result.stderr
    assert not log.exists()


@pytest.mark.release
def test_make_release_targets_have_no_prerequisites() -> None:
    makefile = (Path(__file__).resolve().parent.parent / "Makefile").read_text(
        encoding="utf-8"
    )
    for target in ("release", "release-test"):
        line = next(
            line for line in makefile.splitlines() if line.startswith(f"{target}:")
        )
        before_comment = line.split("##", 1)[0]
        assert before_comment == f"{target}: "


@pytest.mark.release
def test_candidate_rejects_models_and_identity_drift(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    env = _env()
    env["RELEASE_MODEL_PACKAGES"] = "publish"
    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, env, _services(root))
    assert exc.value.failures[0].error == "release model package decision is invalid"

    services = replace(_services(root), git_head=lambda _repo: "b" * 40)
    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)
    assert (
        exc.value.failures[0].error
        == "release source commit does not match EXPECTED_RELEASE_COMMIT"
    )


def test_default_services_have_no_fixture_lane_evidence() -> None:
    services = driver.default_services()

    assert not hasattr(services, "lane_evidence")
    assert (
        services.coordinator_tool_evidence is driver._default_coordinator_tool_evidence
    )


@pytest.mark.release
def test_tool_skew_is_rejected_before_any_build(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    build_called = False

    def build_local_dist(_repo: Path, _include_models: bool) -> None:
        nonlocal build_called
        build_called = True

    tools = {
        lane: pins.fixture_lane_tool_evidence(lane)
        for lane in ("source", "linux-x86_64-musl", "linux-aarch64-musl")
    }
    tools["source"] = {**tools["source"], "rustc": "rustc 0.0.0"}
    services = replace(
        _services(root),
        coordinator_tool_evidence=lambda: tools,
        build_local_dist=build_local_dist,
    )

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert not build_called
    assert any(
        failure.error == "release lane tool rustc is not pinned"
        for failure in exc.value.failures
    )


def test_drift_model_version_is_not_a_publishable_version() -> None:
    with pytest.raises(InvalidVersion):
        Version(DRIFT_MODEL_VERSION)
    assert "-" not in DRIFT_MODEL_VERSION


@pytest.mark.release
def test_models_decision_is_bound_in_ledger_and_recovery(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    env = _env()
    env["RELEASE_MODEL_PACKAGES"] = "include"
    report = driver.run_candidate(root, env, _services(root))
    payload = json.loads((report.evidence_dir / "ledger.json").read_text())

    assert payload["models"] == {
        "decision": "include",
        "package_version": wheel_checker._models_version(),
    }
    assert any(
        item["name"].startswith("solstone_journal_models-")
        for item in payload["candidate"]["files"]
    )

    (root / "packages" / "solstone-journal-models" / "pyproject.toml").write_text(
        "[project]\n"
        'name = "solstone-journal-models"\n'
        f'version = "{DRIFT_MODEL_VERSION}"\n',
        encoding="utf-8",
    )
    recovered = _recover(root)
    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING


def test_default_build_local_dist_package_selection_tracks_workspace_sources() -> None:
    root_data = tomllib.loads(Path("pyproject.toml").read_text(encoding="utf-8"))
    inventory = driver.load_release_package_inventory(driver.ROOT)
    expected_include = tuple(
        sorted({inventory.root_distribution, *inventory.workspace_distributions})
    )

    assert driver.ROOT_WORKSPACE_PACKAGE == root_data["project"]["name"]
    assert driver.MODELS_WORKSPACE_PACKAGE in inventory.workspace_distributions
    assert (
        driver._expected_local_build_packages(include_models=True) == expected_include
    )
    assert driver._expected_local_build_packages(include_models=False) == tuple(
        name for name in expected_include if name != driver.MODELS_WORKSPACE_PACKAGE
    )


@pytest.mark.parametrize(
    ("include_models", "mutation"),
    [
        (False, "unselected"),
        (True, "partial"),
        (True, "changed"),
    ],
)
def test_default_build_local_dist_rejects_models_inventory_drift(
    tmp_path: Path,
    include_models: bool,
    mutation: str,
) -> None:
    _prepare_fake_build_root(tmp_path)

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=include_models,
        )
        if _is_final_core_wheel_build(tmp_path, argv, include_models=include_models):
            dist = tmp_path / "dist"
            if mutation == "unselected":
                for name in (
                    name
                    for name in checker.expected_package_names(include_models=True)
                    if name.startswith("solstone_journal_models-")
                ):
                    (dist / name).write_bytes(b"package")
            elif mutation == "partial":
                models = sorted(
                    path
                    for path in dist.iterdir()
                    if path.name.startswith("solstone_journal_models-")
                )
                models[0].unlink()
            elif mutation == "changed":
                models = sorted(
                    path
                    for path in dist.iterdir()
                    if path.name.startswith("solstone_journal_models-")
                )
                changed = models[0].name.replace(
                    wheel_checker._models_version(), DRIFT_MODEL_VERSION
                )
                models[0].unlink()
                (dist / changed).write_bytes(b"package")
        return subprocess.CompletedProcess(argv, 0, "", "")

    with pytest.raises(driver.DriverError) as exc:
        driver._default_build_local_dist(
            tmp_path,
            include_models=include_models,
            runner=runner,
        )

    assert (
        exc.value.failures[0].error
        == "local release build artifact inventory does not match models decision"
    )


@pytest.mark.parametrize("marker", [b"*", b"*\n"])
def test_default_build_local_dist_strips_uv_dist_gitignore_marker(
    tmp_path: Path,
    marker: bytes,
) -> None:
    _prepare_fake_build_root(tmp_path)

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=False,
        )
        if tuple(argv[:2]) == ("uv", "build"):
            (tmp_path / "dist" / ".gitignore").write_bytes(marker)
        return subprocess.CompletedProcess(argv, 0, "", "")

    try:
        driver._default_build_local_dist(tmp_path, include_models=False, runner=runner)
    except driver.DriverError as exc:
        pytest.fail(
            "; ".join(
                f"{failure.error}: actual={failure.actual}" for failure in exc.failures
            )
        )

    assert not (tmp_path / "dist" / ".gitignore").exists()
    assert {p.name for p in (tmp_path / "dist").iterdir()} == set(
        driver._expected_local_dist_names(include_models=False)
    )


def test_default_build_local_dist_rejects_foreign_dist_gitignore_content(
    tmp_path: Path,
) -> None:
    _prepare_fake_build_root(tmp_path)

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=False,
        )
        if _is_final_core_wheel_build(tmp_path, argv, include_models=False):
            (tmp_path / "dist" / ".gitignore").write_bytes(b"build/")
        return subprocess.CompletedProcess(argv, 0, "", "")

    with pytest.raises(driver.DriverError) as exc:
        driver._default_build_local_dist(tmp_path, include_models=False, runner=runner)

    assert (
        exc.value.failures[0].error
        == "local release build artifact inventory does not match models decision"
    )
    assert ".gitignore" in exc.value.failures[0].actual


def test_default_build_local_dist_rejects_foreign_dist_dotfile(
    tmp_path: Path,
) -> None:
    _prepare_fake_build_root(tmp_path)

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=False,
        )
        if _is_final_core_wheel_build(tmp_path, argv, include_models=False):
            (tmp_path / "dist" / ".hidden").write_bytes(b"")
        return subprocess.CompletedProcess(argv, 0, "", "")

    with pytest.raises(driver.DriverError) as exc:
        driver._default_build_local_dist(tmp_path, include_models=False, runner=runner)

    assert (
        exc.value.failures[0].error
        == "local release build artifact inventory does not match models decision"
    )
    assert ".hidden" in exc.value.failures[0].actual


def test_default_build_local_dist_rejects_symlink_dist_gitignore(
    tmp_path: Path,
) -> None:
    _prepare_fake_build_root(tmp_path)

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=False,
        )
        if _is_final_core_wheel_build(tmp_path, argv, include_models=False):
            (tmp_path / "dist" / ".gitignore").symlink_to("uv-generated-marker")
        return subprocess.CompletedProcess(argv, 0, "", "")

    with pytest.raises(driver.DriverError) as exc:
        driver._default_build_local_dist(tmp_path, include_models=False, runner=runner)

    assert {failure.error for failure in exc.value.failures} >= {
        "local release build produced unsafe dist entry"
    }


def test_default_build_local_dist_uses_exact_linux_contract_and_scrubbed_env(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _prepare_fake_build_root(tmp_path)
    monkeypatch.setenv("AMBIENT_RELEASE_TOKEN", "do-not-copy")
    calls: list[tuple[tuple[str, ...], dict[str, str]]] = []

    def runner(
        argv: Sequence[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = kwargs.get("env")
        assert isinstance(env, dict)
        calls.append((tuple(argv), dict(env)))
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=False,
        )
        return subprocess.CompletedProcess(argv, 0, "", "")

    driver._default_build_local_dist(tmp_path, include_models=False, runner=runner)

    expected_commands = driver._expected_local_build_commands(
        include_models=False,
        version=checker._current_version(),
    )
    assert calls == [
        (argv, _expected_scrubbed_env(tmp_path, maturin_args, ort_target))
        for argv, maturin_args, ort_target in expected_commands
    ]
    assert all("--exclude" not in argv for argv, _env in calls)
    assert [env["MATURIN_PEP517_ARGS"] for argv, env in calls if "--wheel" in argv] == [
        maturin_args
        for argv, maturin_args, _ort_target in expected_commands
        if "--wheel" in argv
    ]
    assert all("AMBIENT_RELEASE_TOKEN" not in env for _argv, env in calls)
    helper_build_argv = (
        "uv",
        "build",
        "--package",
        driver.SPEAKERS_ANALYZE_WORKSPACE_PACKAGE,
        "--wheel",
    )
    assert all(
        not any(key.startswith("ORT_") for key in env) or argv == helper_build_argv
        for argv, env in calls
    )
    for _argv, env in calls:
        assert Path(env["ZIG_GLOBAL_CACHE_DIR"]).is_relative_to(tmp_path)
        assert Path(env["ZIG_LOCAL_CACHE_DIR"]).is_relative_to(tmp_path)
    assert {path.name for path in (tmp_path / "dist").iterdir()} == set(
        driver._expected_local_dist_names(include_models=False)
    )


def test_scrubbed_build_env_reports_uncreatable_zig_cache_root(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / "target" / "release-zig-cache"
    cache_root.parent.mkdir(parents=True)
    cache_root.write_text("not a directory", encoding="utf-8")

    with pytest.raises(driver.DriverError) as exc:
        driver._scrubbed_build_env(tmp_path, driver.CORE_X86_64_MATURIN_ARGS, None)

    assert exc.value.failures[0].error == (
        "release Zig cache directory could not be created"
    )
    assert exc.value.failures[0].expected == (
        "writable Zig cache directories under target/release-zig-cache"
    )
    assert "NotADirectoryError" in exc.value.failures[0].actual


def test_scrubbed_build_env_rejects_unknown_ort_target(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="unknown speakers-analyze ORT target"):
        driver._scrubbed_build_env(
            tmp_path,
            driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS,
            "not-a-target",
        )


def test_default_build_local_dist_honors_include_models_build_selection(
    tmp_path: Path,
) -> None:
    _prepare_fake_build_root(tmp_path)
    calls: list[tuple[str, ...]] = []

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        calls.append(tuple(argv))
        _fabricate_local_dist_for_build_argv(
            tmp_path,
            argv,
            include_models=True,
        )
        return subprocess.CompletedProcess(argv, 0, "", "")

    driver._default_build_local_dist(tmp_path, include_models=True, runner=runner)

    assert calls == [
        argv
        for argv, _maturin_args, _ort_target in driver._expected_local_build_commands(
            include_models=True,
            version=checker._current_version(),
        )
    ]
    assert all("--exclude" not in call for call in calls)
    assert {path.name for path in (tmp_path / "dist").iterdir()} == set(
        driver._expected_local_dist_names(include_models=True)
    )


@pytest.mark.parametrize("include_models", [False, True])
def test_local_dist_inventory_accepts_owned_reserved_candidate_parent_for_both_model_decisions(
    tmp_path: Path,
    include_models: bool,
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=include_models)
    reserved = _reserved_candidate_path(root)
    (reserved / "retained" / "marker.txt").parent.mkdir(parents=True)
    (reserved / "retained" / "marker.txt").write_text("keep", encoding="utf-8")
    before = _structural_snapshot(reserved)

    driver._validate_local_dist_inventory(root / "dist", include_models=include_models)

    assert _structural_snapshot(reserved) == before


def test_local_dist_inventory_does_not_enumerate_reserved_candidate_parent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)
    dist = root / "dist"
    reserved = _reserved_candidate_path(root)
    (reserved / "retained" / "marker.txt").parent.mkdir(parents=True)
    (reserved / "retained" / "marker.txt").write_text("keep", encoding="utf-8")
    original_listdir = os.listdir
    original_scandir = os.scandir
    recorded: list[Path] = []

    def record(path: object) -> None:
        if isinstance(path, int):
            return
        try:
            recorded.append(Path(path))
        except TypeError:
            return

    def listdir(path: object = ".") -> list[str]:
        record(path)
        return original_listdir(path)

    def scandir(path: object = ".") -> object:
        record(path)
        return original_scandir(path)

    monkeypatch.setattr(os, "listdir", listdir)
    monkeypatch.setattr(os, "scandir", scandir)

    driver._validate_local_dist_inventory(dist, include_models=False)

    assert dist in recorded
    assert all(
        path != reserved and not path.is_relative_to(reserved) for path in recorded
    )


def test_local_dist_inventory_accepts_first_release_without_reserved_candidate_parent(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)

    driver._validate_local_dist_inventory(root / "dist", include_models=False)


def test_local_dist_inventory_rejects_denied_dist_read_search(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)
    dist = root / "dist"
    facts_before = _existing_expected_artifact_facts(root, include_models=False)
    access_calls = _access_spy(monkeypatch, denied_path=dist)

    with pytest.raises(driver.DriverError) as exc:
        driver._validate_local_dist_inventory(dist, include_models=False)

    _assert_dist_preflight_failure(
        exc,
        operation="inventory",
        actual="dist/ lacks read/search access",
        denied_access=True,
    )
    assert access_calls == [(dist, os.R_OK | os.X_OK)]
    assert _existing_expected_artifact_facts(root, include_models=False) == facts_before


def test_fresh_cleanup_preserves_retained_candidates_and_removes_only_current_candidate_transients(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    reserved = _reserved_candidate_path(root)
    prior_names = (
        PRIOR_RETAINED_VERSION,
        f"{version}0",
        f"{version}0.payload-staging",
    )
    for name in prior_names:
        marker = reserved / name / "nested" / "marker.txt"
        marker.parent.mkdir(parents=True)
        marker.write_text(f"keep {name}", encoding="utf-8")
    retained_before = {
        name: _structural_snapshot(reserved / name) for name in prior_names
    }
    lock = reserved / ".rust-release-candidate.lock"
    lock.write_text("lock", encoding="utf-8")
    lock_before = lock.read_bytes()
    current_paths = (
        reserved / version,
        reserved / f"{version}.payload-staging",
        reserved / f"{version}.payload-staging.staging",
        reserved / f"{version}.payload-staging.quarantine",
    )
    for path in current_paths:
        (path / "marker.txt").parent.mkdir(parents=True)
        (path / "marker.txt").write_text("stale", encoding="utf-8")
    _write_expected_local_dist(root, include_models=False)
    raw_artifacts = tuple(
        root / "dist" / name
        for name in driver._expected_local_dist_names(include_models=False)
    )

    driver._default_clean_outputs(root, version)

    retained_after_cleanup = {
        name: _structural_snapshot(reserved / name) for name in prior_names
    }
    assert retained_after_cleanup == retained_before
    assert lock.read_bytes() == lock_before
    assert all(not path.exists() for path in current_paths)
    assert all(not path.exists() for path in raw_artifacts)

    _write_expected_local_dist(root, include_models=False)
    driver._validate_local_dist_inventory(root / "dist", include_models=False)
    assert {name: _structural_snapshot(reserved / name) for name in prior_names} == (
        retained_before
    )


@pytest.mark.parametrize(
    ("reserved_kind", "actual"),
    [
        ("symlink", "dist/release-candidate is symlink"),
        ("regular", "dist/release-candidate is regular file"),
        ("fifo", "dist/release-candidate is special file"),
        (
            "denied-directory",
            "dist/release-candidate lacks write/search access",
        ),
    ],
)
def test_fresh_cleanup_rejects_unsafe_reserved_candidate_parent_before_any_mutation(
    tmp_path: Path,
    reserved_kind: str,
    actual: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    build_keep = root / "build" / "keep"
    build_keep.parent.mkdir()
    build_keep.write_text("keep", encoding="utf-8")
    reserved = _reserved_candidate_path(root)
    reserved.parent.mkdir(parents=True)
    outside = tmp_path / "outside"
    outside_before: tuple[TreeSnapshotEntry, ...] | None = None
    if reserved_kind == "symlink":
        marker = outside / version / "nested" / "keep.txt"
        marker.parent.mkdir(parents=True)
        marker.write_text("keep", encoding="utf-8")
        outside_before = _structural_snapshot(outside)
        reserved.symlink_to(outside, target_is_directory=True)
    elif reserved_kind == "regular":
        reserved.write_text("unsafe", encoding="utf-8")
    elif reserved_kind == "fifo":
        os.mkfifo(reserved)
    elif reserved_kind == "denied-directory":
        reserved.mkdir()

    access_calls = _access_spy(
        monkeypatch,
        denied_path=reserved if reserved_kind == "denied-directory" else None,
    )

    with pytest.raises(driver.DriverError) as exc:
        driver._default_clean_outputs(root, version)

    denied_access = reserved_kind == "denied-directory"
    _assert_reserved_parent_failure(
        exc,
        operation="cleanup",
        actual=actual,
        denied_access=denied_access,
    )
    expected_access_calls = [(root / "dist", os.R_OK | os.W_OK | os.X_OK)]
    if denied_access:
        expected_access_calls.append((reserved, os.W_OK | os.X_OK))
    assert access_calls == expected_access_calls
    assert build_keep.read_text(encoding="utf-8") == "keep"
    if reserved_kind == "symlink":
        assert reserved.is_symlink()
        assert outside_before is not None
        assert _structural_snapshot(outside) == outside_before


@pytest.mark.parametrize(
    ("dist_kind", "actual"),
    [
        ("symlink", "dist/ is symlink"),
        ("regular", "dist/ is regular file"),
        ("fifo", "dist/ is special file"),
        ("denied-directory", "dist/ lacks read/write/search access"),
    ],
)
def test_fresh_cleanup_rejects_unsafe_dist_before_any_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    dist_kind: str,
    actual: str,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    dist = root / "dist"
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "external-target" / "marker.txt").parent.mkdir()
    (outside / "external-target" / "marker.txt").write_text(
        "external", encoding="utf-8"
    )
    if dist_kind == "symlink":
        _write_cleanup_preflight_sentinels(root, outside, version=version)
        dist.symlink_to(outside, target_is_directory=True)
    elif dist_kind == "regular":
        _write_cleanup_preflight_sentinels(root, None, version=version)
        dist.write_text("unsafe", encoding="utf-8")
    elif dist_kind == "fifo":
        _write_cleanup_preflight_sentinels(root, None, version=version)
        os.mkfifo(dist)
    elif dist_kind == "denied-directory":
        _write_cleanup_preflight_sentinels(root, dist, version=version)

    root_before = _structural_snapshot(root)
    outside_before = _structural_snapshot(outside)
    blocked_calls: list[str] = []

    def fail_if_called(name: str) -> object:
        def wrapper(*_args: object, **_kwargs: object) -> object:
            blocked_calls.append(name)
            raise AssertionError(f"{name} must not run after failed dist preflight")

        return wrapper

    for name in (
        "_remove_owned_path",
        "_remove_owned_relative",
        "_owned_glob",
        "_clean_raw_dist_outputs",
        "_payload_transient_paths",
    ):
        monkeypatch.setattr(driver, name, fail_if_called(name))
    access_calls = _access_spy(
        monkeypatch,
        denied_path=dist if dist_kind == "denied-directory" else None,
    )
    with monkeypatch.context() as enumeration_patch:
        enumerated = _enumeration_spy(enumeration_patch)

        with pytest.raises(driver.DriverError) as exc:
            driver._default_clean_outputs(root, version)

        assert blocked_calls == []
        assert all(not _same_or_descendant(path, dist) for path in enumerated)

    _assert_dist_preflight_failure(
        exc,
        operation="cleanup",
        actual=actual,
        denied_access=dist_kind == "denied-directory",
    )
    expected_access_calls: list[tuple[Path, int]] = []
    if dist_kind == "denied-directory":
        expected_access_calls.append((dist, os.R_OK | os.W_OK | os.X_OK))
    assert access_calls == expected_access_calls
    assert _structural_snapshot(root) == root_before
    assert _structural_snapshot(outside) == outside_before


@pytest.mark.parametrize(
    ("reserved_kind", "actual"),
    [
        ("symlink", "dist/release-candidate is symlink"),
        ("regular", "dist/release-candidate is regular file"),
        ("fifo", "dist/release-candidate is special file"),
    ],
)
def test_local_dist_inventory_rejects_unsafe_reserved_candidate_parent_without_mutating_artifacts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    reserved_kind: str,
    actual: str,
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)
    facts_before = _existing_expected_artifact_facts(root, include_models=False)
    reserved = _reserved_candidate_path(root)
    outside = tmp_path / "outside"
    outside_before: tuple[TreeSnapshotEntry, ...] | None = None
    if reserved_kind == "symlink":
        marker = outside / "nested" / "keep.txt"
        marker.parent.mkdir(parents=True)
        marker.write_text("keep", encoding="utf-8")
        outside_before = _structural_snapshot(outside)
        reserved.symlink_to(outside, target_is_directory=True)
    elif reserved_kind == "regular":
        reserved.write_text("unsafe", encoding="utf-8")
    elif reserved_kind == "fifo":
        os.mkfifo(reserved)
    access_calls = _access_spy(monkeypatch)

    with pytest.raises(driver.DriverError) as exc:
        driver._validate_local_dist_inventory(root / "dist", include_models=False)

    _assert_reserved_parent_failure(exc, operation="inventory", actual=actual)
    assert access_calls == [(root / "dist", os.R_OK | os.X_OK)]
    assert _existing_expected_artifact_facts(root, include_models=False) == facts_before
    if reserved_kind == "symlink":
        assert reserved.is_symlink()
        assert outside_before is not None
        assert _structural_snapshot(outside) == outside_before


def test_local_dist_inventory_accepts_reserved_directory_without_reserved_access(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)
    reserved = _reserved_candidate_path(root)
    marker = reserved / "retained" / "marker.txt"
    marker.parent.mkdir(parents=True)
    marker.write_text("keep", encoding="utf-8")
    reserved.chmod(0o700)
    before = _structural_snapshot(reserved)
    access_calls = _access_spy(monkeypatch, denied_path=reserved)

    try:
        reserved.chmod(0)
        driver._validate_local_dist_inventory(root / "dist", include_models=False)
    finally:
        reserved.chmod(0o700)

    assert access_calls == [(root / "dist", os.R_OK | os.X_OK)]
    assert _structural_snapshot(reserved) == before


@pytest.mark.parametrize(
    "mutation",
    [
        "foreign-directory",
        "foreign-symlink",
        "foreign-special",
        "extra-regular-file",
        "missing-expected-file",
        "wrong-model-package-set",
    ],
)
def test_local_dist_inventory_rejects_foreign_entries_with_reserved_candidate_parent_present(
    tmp_path: Path,
    mutation: str,
) -> None:
    root = _repo(tmp_path)
    _write_expected_local_dist(root, include_models=False)
    reserved = _reserved_candidate_path(root)
    reserved.mkdir(parents=True)
    expected = driver._expected_local_dist_names(include_models=False)
    if mutation == "foreign-directory":
        (root / "dist" / "foreign-dir").mkdir()
    elif mutation == "foreign-symlink":
        (root / "dist" / "foreign-link").symlink_to(next(iter(sorted(expected))))
    elif mutation == "foreign-special":
        os.mkfifo(root / "dist" / "foreign-pipe")
    elif mutation == "extra-regular-file":
        (root / "dist" / "extra.whl").write_bytes(b"extra")
    elif mutation == "missing-expected-file":
        missing = next(
            name
            for name in sorted(expected)
            if name.startswith("solstone-") and name.endswith(".tar.gz")
        )
        (root / "dist" / missing).unlink()
    elif mutation == "wrong-model-package-set":
        extra_models = driver._expected_local_dist_names(include_models=True) - expected
        for name in extra_models:
            (root / "dist" / name).write_bytes(b"model")
    facts_before = _existing_expected_artifact_facts(root, include_models=False)

    with pytest.raises(driver.DriverError):
        driver._validate_local_dist_inventory(root / "dist", include_models=False)

    assert _existing_expected_artifact_facts(root, include_models=False) == facts_before


def test_fresh_cleanup_removes_nested_egg_infos_request_siblings_and_staging(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    version = checker._current_version()
    paths = [
        root / "packages" / "solstone-journal" / "solstone_journal.egg-info",
        root / "packages" / "solstone-journal-cuda" / "solstone_journal_cuda.egg-info",
        root
        / "packages"
        / "solstone-journal-models"
        / "solstone_journal_models.egg-info",
        root
        / "packages"
        / driver.SPEAKERS_ANALYZE_WORKSPACE_PACKAGE
        / "solstone_core_speakers_analyze.egg-info",
        root / "packages" / driver.SPEAKERS_ANALYZE_WORKSPACE_PACKAGE / "wheel-data",
        root / "target" / "release-transfer" / f".{version}.request-abc123",
        root / "target" / "release-transfer" / f".{version}.source.bundle",
        root / "target" / "release-evidence" / f"{version}.staging",
        root / "target" / "release-zig-cache",
        root / "dist" / "release-candidate" / f"{version}.payload-staging.staging",
        root / "dist" / "release-candidate" / f"{version}.payload-staging.quarantine",
    ]
    for path in paths:
        path.mkdir(parents=True, exist_ok=True)
        (path / "marker").write_text("stale", encoding="utf-8")

    driver._default_clean_outputs(root, version)

    assert all(not path.exists() for path in paths)


@pytest.mark.parametrize("relative", ["packages/solstone-journal/bad.egg-info", "dist"])
def test_fresh_cleanup_preserves_symlink_targets_and_surfaces_residue(
    tmp_path: Path, relative: str
) -> None:
    root = _repo(tmp_path)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep.txt").write_text("keep", encoding="utf-8")
    link = root / relative
    link.parent.mkdir(parents=True, exist_ok=True)
    if link.exists() or link.is_symlink():
        link.unlink()
    link.symlink_to(outside, target_is_directory=True)

    with pytest.raises(driver.DriverError) as exc:
        driver._default_clean_outputs(root, checker._current_version())

    assert (outside / "keep.txt").read_text(encoding="utf-8") == "keep"
    assert link.is_symlink()
    if relative == "dist":
        _assert_dist_preflight_failure(
            exc,
            operation="cleanup",
            actual="dist/ is symlink",
        )
    else:
        assert any("symlink residue" in failure.error for failure in exc.value.failures)


@pytest.mark.parametrize(
    "args",
    [
        driver.CORE_X86_64_MATURIN_ARGS.replace("--locked ", ""),
        driver.CORE_X86_64_MATURIN_ARGS.replace("--zig ", ""),
        driver.CORE_X86_64_MATURIN_ARGS.replace("--compatibility manylinux2014 ", ""),
        driver.CORE_X86_64_MATURIN_ARGS.replace(
            "--compatibility manylinux2014", "--compatibility manylinux_2_28"
        ),
        driver.CORE_X86_64_MATURIN_ARGS.replace(
            "--target x86_64-unknown-linux-musl", ""
        ),
        driver.CORE_X86_64_MATURIN_ARGS.replace(
            "x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu"
        ),
    ],
)
def test_linux_maturin_contract_rejects_missing_or_wrong_tokens(args: str) -> None:
    failures = driver.validate_linux_maturin_args(
        args,
        target="x86_64-unknown-linux-musl",
    )
    assert failures


@pytest.mark.parametrize(
    "args",
    [
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace("--locked ", ""),
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace("--zig ", ""),
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace(
            "--compatibility manylinux_2_27 ", ""
        ),
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace("--auditwheel skip ", ""),
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace(
            "--target x86_64-unknown-linux-gnu", ""
        ),
        driver.SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS.replace(
            "x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"
        ),
    ],
)
def test_speakers_analyze_linux_maturin_contract_rejects_missing_or_wrong_tokens(
    args: str,
) -> None:
    failures = driver.validate_speakers_analyze_linux_maturin_args(
        args,
        target="x86_64-unknown-linux-gnu",
    )
    assert failures


@pytest.mark.parametrize(
    "mutation",
    [
        "record_role",
        "record_paths_swapped",
        "wrong_tag",
        "member",
        "tool",
        "signing",
        "notary",
        "wheel_hash",
    ],
)
@pytest.mark.release
def test_candidate_rejects_native_record_mismatches(
    tmp_path: Path, mutation: str
) -> None:
    root = _repo(tmp_path)

    with pytest.raises(driver.DriverError):
        driver.run_candidate(root, _env(), _services(root, native_mutation=mutation))


@pytest.mark.release
def test_candidate_revalidates_macos_wheel_bytes_after_copy_before_ledger(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    services = _services(root)

    def cleanup(paths: Sequence[Path]) -> None:
        root_name, _core_name, _speakers_name = _macos_wheel_names()
        wheel_path = root / "dist" / root_name
        if wheel_path.exists():
            write_platform_base_wheel(
                wheel_path.parent,
                helper_binary=b"mutated-helper",
                version=checker._current_version(),
            )
        for path in paths:
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists() or path.is_symlink():
                path.unlink()

    services = replace(services, cleanup_transients=cleanup)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert any(
        failure.error == "release candidate wheel content check failed"
        and PARAKEET_HELPER_MEMBER in failure.actual
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_candidate_rejects_poisoned_core_wheel_content(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    base_services = _services(root)

    def build_local_dist(repo: Path, include_models: bool) -> None:
        base_services.build_local_dist(repo, include_models)
        write_core_wheel(
            repo / "dist",
            tag="manylinux_2_17_x86_64.manylinux2014_x86_64",
            binary=b"not an elf",
            version=checker._current_version(),
        )

    services = replace(base_services, build_local_dist=build_local_dist)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    _assert_no_ready_cohort(root)
    assert any(
        failure.error == "release candidate wheel content check failed"
        and ".data/scripts/solstone-core" in failure.actual
        and "ELF binary is too short" in failure.actual
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_candidate_rejects_coordinator_sourced_macos_tool_evidence(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    services = _services(root)
    base = services.coordinator_tool_evidence()
    services = replace(
        services,
        coordinator_tool_evidence=lambda: {
            **base,
            "macos-arm64": pins.fixture_lane_tool_evidence("macos-arm64"),
        },
    )

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert (
        exc.value.failures[0].error
        == "macOS release tool evidence must be attested by the build host"
    )


@pytest.mark.release
def test_candidate_rejects_forged_host_macos_tool_evidence(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    services = _services(root)

    def build_host(
        source_bundle: SourceBundle, commit: str, output_dir: Path
    ) -> BuildHostResult:
        result = _write_macos_host_outputs(output_dir)
        tools = dict(result.tool_evidence)
        tools["swift"] = "Apple Swift 6.3.3"
        return BuildHostResult(
            macos_wheels=result.macos_wheels,
            native_records=result.native_records,
            tool_evidence=tools,
        )

    services = replace(services, build_host=build_host)

    with pytest.raises(driver.DriverError) as exc:
        driver.run_candidate(root, _env(), services)

    assert any(
        failure.error == "pre-sign lane tool swift is not pinned"
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_candidate_derives_manifest_evidence_from_single_frozen_tool_observation(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    calls = 0

    def coordinator_tool_evidence() -> dict[str, dict[str, str]]:
        nonlocal calls
        calls += 1
        return {
            lane: pins.fixture_lane_tool_evidence(lane)
            for lane in ("source", "linux-x86_64-musl", "linux-aarch64-musl")
        }

    services = replace(
        _services(root), coordinator_tool_evidence=coordinator_tool_evidence
    )
    report = driver.run_candidate(root, _env(), services)
    payload = json.loads((report.evidence_dir / "ledger.json").read_text())

    assert calls == 1
    assert payload["tool_evidence"]["source"]["uv"] == pins.UV_LINUX_FIXTURE_BANNER
    assert payload["tool_evidence"]["source"]["maturin"] == pins.MATURIN_PIN
    source_artifact = next(
        name
        for name, (lane, _target) in checker.rust_artifact_targets().items()
        if lane == "source"
    )
    manifest = json.loads(
        (
            report.release_dir / f"{source_artifact}.rust-release-manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert manifest["native_tools"]["uv"] == pins.UV_LINUX_FIXTURE_BANNER


@pytest.mark.release
def test_recovery_rejects_native_member_path_mutation_with_matching_hash(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger_path = report.evidence_dir / "ledger.json"
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    payload["native_members"]["linux-x86_64-musl"]["solstone-core"]["path"] = (
        "forged/path/solstone-core"
    )
    ledger_path.write_bytes(checker.canonical_json_bytes(payload))

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert (
        exc.value.failures[0].error
        == "retained ledger native_members do not match finalized wheels"
    )


def test_proof_binding_surfaces_target_install_parse_failure(tmp_path: Path) -> None:
    target = "linux-x86_64-musl"
    digest = "a" * 64
    ledger_sha256 = "b" * 64
    ledger_payload = {
        "source_commit": SOURCE_COMMIT,
        "core_lock_sha256": LOCK_SHA,
        "candidate": {"files": []},
        "native_members": {
            target: {
                "solstone-core": {
                    "path": "solstone_core-0.9.0.data/scripts/solstone-core",
                    "sha256": "d" * 64,
                    "bytes": 5,
                }
            }
        },
    }
    proof = {
        "schema_version": driver.CURRENT_PROOF_SCHEMA_VERSION,
        "target": target,
        "source_commit": SOURCE_COMMIT,
        "candidate_digest": digest,
        "ledger_sha256": ledger_sha256,
        "core_lock_sha256": LOCK_SHA,
        "candidate_files": [],
        "installed_members": [
            {
                "name": "solstone-core",
                "wheel_member_path": ("solstone_core-0.9.0.data/scripts/solstone-core"),
                "installed_path": "ENVROOT/bin/solstone-core",
                "sha256": "d" * 64,
            }
        ],
    }

    failures = driver._validate_proof_binding(
        proof,
        target=target,
        ledger=ledger_payload,
        digest=digest,
        ledger_sha256=ledger_sha256,
        release_dir=tmp_path,
    )

    assert any(
        failure.error == "install proof target install set is empty"
        for failure in failures
    )


@pytest.mark.release
def test_recovery_rejects_empty_linux_native_member_set(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger_path = report.evidence_dir / "ledger.json"
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    payload["native_members"]["linux-x86_64-musl"] = {}
    ledger_path.write_bytes(checker.canonical_json_bytes(payload))

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert any(
        "native member set is invalid" in failure.error
        for failure in exc.value.failures
    )


@pytest.mark.release
def test_recovery_rejects_self_consistent_native_member_forgery(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger_path = report.evidence_dir / "ledger.json"
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    forged = payload["native_members"]["linux-x86_64-musl"]["solstone-core"]
    forged["path"] = "forged/path/solstone-core"
    forged["sha256"] = "0" * 64
    ledger_path.write_bytes(checker.canonical_json_bytes(payload))
    forged_ledger_sha = hashlib.sha256(ledger_path.read_bytes()).hexdigest()

    for proof_path in sorted((report.evidence_dir / "proofs").glob("*.json")):
        proof = json.loads(proof_path.read_text(encoding="utf-8"))
        proof["ledger_sha256"] = forged_ledger_sha
        if proof["target"] == "linux-x86_64-musl":
            proof["installed_members"] = [
                {
                    "name": name,
                    "wheel_member_path": member["path"],
                    "installed_path": f"ENVROOT/bin/{name}",
                    "sha256": member["sha256"],
                }
                for name, member in sorted(
                    payload["native_members"]["linux-x86_64-musl"].items()
                )
            ]
        proof_path.write_bytes(checker.canonical_json_bytes(proof))

    with pytest.raises(driver.DriverError) as exc:
        _recover(root)

    assert (
        exc.value.failures[0].error
        == "retained ledger native_members do not match finalized wheels"
    )


@pytest.mark.release
def test_recovery_rejects_self_consistent_nvattest_authority_forgery(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    before_payload = _structural_snapshot(report.release_dir)
    ledger = _read_json(_ledger_path(report))
    nvattest = ledger["nvattest"]
    forged_authority = copy.deepcopy(nvattest["authority"])
    assert isinstance(forged_authority, dict)
    for target_key, authority_target in sorted(forged_authority["targets"].items()):
        authority_target["artifact"]["sha256"] = hashlib.sha256(
            f"forged authority {target_key}".encode("utf-8")
        ).hexdigest()
    forged_authority_bytes = driver._canonical_nvattest_authority_bytes(
        forged_authority
    )
    forged_challenge = hashlib.sha256(
        f"forged challenge {report.version}".encode("utf-8")
    ).hexdigest()
    assert CHALLENGE_RE.fullmatch(forged_challenge)
    nvattest["challenge"] = forged_challenge
    nvattest["authority"] = forged_authority
    nvattest["authority_sha256"] = hashlib.sha256(forged_authority_bytes).hexdigest()
    nvattest["support_distributions"] = _retained_support_declarations(report)
    forged_ledger_sha = _write_ledger(report, ledger)
    _update_retained_receipt_ledger_sha(report, forged_ledger_sha)

    for target in driver.PROOF_TARGETS:
        receipt_path = _nvattest_receipt_path(report, target)
        receipt = _read_json(receipt_path)
        _sync_nvattest_receipt_with_ledger(
            receipt,
            nvattest=nvattest,
            ledger_sha256=forged_ledger_sha,
        )
        _write_json(receipt_path, receipt)

    assert _structural_snapshot(report.release_dir) == before_payload
    _assert_fails_with_error(
        root,
        "retained nvattest authority disagrees with candidate wheels",
    )


@pytest.mark.release
def test_recovery_success_preserves_retained_tree_and_uses_no_seams(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    services = _services(root)
    report = driver.run_candidate(root, _env(), services)
    before_payload = _structural_snapshot(report.release_dir)
    before_evidence = _structural_snapshot(report.evidence_dir)
    _reset_service_call_counts(services)

    recovered = _recover(root)

    assert recovered.heading == driver.RETAINED_CANDIDATE_VALID_HEADING
    assert _structural_snapshot(report.release_dir) == before_payload
    assert _structural_snapshot(report.evidence_dir) == before_evidence
    _assert_service_call_counts_zero(services)


@pytest.mark.release
def test_recovery_failure_preserves_retained_tree_and_uses_no_seams(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    services = _services(root)
    report = driver.run_candidate(root, _env(), services)
    _nvattest_receipt_path(report, driver.PROOF_TARGETS[0]).unlink()

    _assert_recovery_failure_preserves_retained_tree(
        root,
        report,
        "release nvattest inventory is not exact",
        services,
    )


@pytest.mark.parametrize(
    ("mutation", "expected_error"),
    [
        ("extra", "release nvattest inventory is not exact"),
        ("duplicate", "nvattest proof target is not bound to expected input"),
        ("swapped", "nvattest proof target is not bound to expected input"),
        ("stale", "nvattest proof version is not bound to expected input"),
        ("replayed", "nvattest proof challenge is not bound to expected input"),
        ("mutated", "nvattest proof kind is invalid"),
        ("noncanonical", "nvattest proof bytes are not canonical"),
    ],
)
@pytest.mark.release
def test_recovery_rejects_retained_nvattest_receipt_mutations(
    tmp_path: Path,
    mutation: str,
    expected_error: str,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    targets = tuple(driver.PROOF_TARGETS)
    first_target = targets[0]
    second_target = targets[1]
    first_path = _nvattest_receipt_path(report, first_target)
    second_path = _nvattest_receipt_path(report, second_target)

    if mutation == "extra":
        extra_path = report.evidence_dir / "nvattest" / "extra.json"
        extra_path.write_bytes(first_path.read_bytes())
    elif mutation == "duplicate":
        second_path.write_bytes(first_path.read_bytes())
    elif mutation == "swapped":
        first_bytes = first_path.read_bytes()
        second_bytes = second_path.read_bytes()
        first_path.write_bytes(second_bytes)
        second_path.write_bytes(first_bytes)
    elif mutation == "stale":
        receipt = _read_json(first_path)
        receipt["version"] = f"{report.version}+stale"
        _write_json(first_path, receipt)
    elif mutation == "replayed":
        replay_root = _repo(tmp_path / "replayed")
        replay_report = driver.run_candidate(
            replay_root, _env(), _services(replay_root)
        )
        replayed_challenge = _read_json(_ledger_path(replay_report))["nvattest"][
            "challenge"
        ]
        assert CHALLENGE_RE.fullmatch(replayed_challenge)
        assert (
            replayed_challenge
            != _read_json(_ledger_path(report))["nvattest"]["challenge"]
        )
        receipt = _read_json(first_path)
        receipt["challenge"] = replayed_challenge
        _write_json(first_path, receipt)
    elif mutation == "mutated":
        receipt = _read_json(first_path)
        receipt["kind"] = "mutated-nvattest-receipt"
        _write_json(first_path, receipt)
    elif mutation == "noncanonical":
        receipt = _read_json(first_path)
        first_path.write_text(
            json.dumps(receipt, indent=2, sort_keys=True), encoding="utf-8"
        )
    else:
        raise AssertionError(f"unknown mutation {mutation}")

    _assert_fails_with_error(root, expected_error)


@pytest.mark.release
def test_recovery_rejects_retained_nvattest_wrong_challenge(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger = _read_json(_ledger_path(report))
    invalid_challenge = "not-a-valid-nvattest-challenge"
    assert not CHALLENGE_RE.fullmatch(invalid_challenge)
    ledger["nvattest"]["challenge"] = invalid_challenge
    ledger_sha = _write_ledger(report, ledger)
    _update_retained_receipt_ledger_sha(report, ledger_sha)

    _assert_fails_with_error(root, "retained ledger nvattest challenge is invalid")


@pytest.mark.release
def test_recovery_rejects_retained_nvattest_support_wheel_byte_mutation(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    support_path = sorted((report.evidence_dir / "support").glob("*.whl"))[0]
    support_path.write_bytes(support_path.read_bytes() + b"\nmutated support bytes\n")

    _assert_fails_with_error(
        root,
        "retained nvattest support bytes disagree with ledger",
    )


@pytest.mark.release
def test_recovery_rejects_retained_nvattest_support_declaration_mutation(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    report = driver.run_candidate(root, _env(), _services(root))
    ledger = _read_json(_ledger_path(report))
    declarations = ledger["nvattest"]["support_distributions"]
    assert {
        entry["name"] for entry in declarations
    } == driver.SUPPORT_DISTRIBUTION_NAMES
    declarations[0]["sha256"] = hashlib.sha256(
        f"forged support declaration {declarations[0]['name']}".encode("utf-8")
    ).hexdigest()
    ledger_sha = _write_ledger(report, ledger)
    _update_retained_receipt_ledger_sha(report, ledger_sha)

    _assert_fails_with_error(
        root,
        "retained nvattest support bytes disagree with ledger",
    )
