#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Provider-neutral release-candidate finalizer driver."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shlex
import shutil
import stat
import subprocess
import sys
import tomllib
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.check_release_preflight import (
    LANE_TOOL_KEYS,
    check_lane_tool_evidence,
    collect_lane_tool_evidence,
    finalize_macos_tool_evidence,
)
from scripts.check_rust_release_manifest import (
    CORE_UNSUPPORTED_TOMBSTONE_RECORD,
    LANES,
    NATIVE_TOOL_KEYS,
    SOURCE_COMMIT_RE,
    Failure,
    LaneEvidence,
    _format_failures,
    build_and_promote_candidate,
    canonical_json_bytes,
    expected_package_names,
    rust_artifact_targets,
    validate_core_unsupported_tombstone_record,
    validate_release_dir,
)
from scripts.check_wheel_contents import (
    EXPECTED_MODEL_SHA256,
    MAX_BASE_WHEEL_BYTES,
    NVATTEST_AUTHORITY_MEMBER,
    check_dist,
)
from scripts.normalize_maturin_sdist import (
    SdistLockError,
    normalize_core_sdist_workspace_lock,
)
from scripts.record_macos_native_wheel import validate_macos_native_record
from scripts.release_advisory_policy import (
    PolicyRun,
    is_normalized_utc_timestamp,
    prepare_policy_run,
    validate_snapshot_identity,
)
from scripts.release_build_host import (
    BuildHostResult,
    ExternalBuildHostChannel,
    SourceBundle,
    create_source_bundle,
)
from scripts.release_digest import bundle_digest, candidate_digest, file_sha256_size
from scripts.release_install_smoke import (
    CANDIDATE,
    CURRENT_PROOF_SCHEMA_VERSION,
    ENVROOT,
    PROOF_TARGETS,
    RETAINED_PROOF_REPAIR,
    InstallProofError,
    _expected_install_members,
    _proof_schema_version,
    _select_names_for_target,
    candidate_file_entries,
    target_install_paths_from_ledger,
    validate_install_proof_bytes,
)
from scripts.release_ledger import (
    LedgerError,
    RetainedLedgerSchema,
    current_retained_ledger_schema,
    read_retained_ledger,
    resolve_retained_ledger_schema,
    retained_ledger_schema_declares_nvattest,
    validate_native_members_against_release_dir,
    write_ledger,
)
from scripts.release_nvattest_proof import (
    CHALLENGE_RE,
    SUPPORT_DISTRIBUTION_NAMES,
    NvattestProofError,
    candidate_wheel_entries,
    support_distribution_entries,
    support_distribution_entries_with_metadata,
    validate_nvattest_proof_bytes,
)
from scripts.release_nvattest_support import (
    SupportLockEntry,
    SupportLockError,
    read_support_lock_entries,
    support_declarations_from_lock,
    validate_support_declarations,
    verify_support_wheels_against_lock,
)
from scripts.release_package_inventory import (  # noqa: E402
    load_release_package_inventory,
    macos_native_record_name,
    native_role,
    normalized_distribution,
)
from scripts.release_proof_host import (
    ProofHostError,
    TargetProofPaths,
    proof_channels_from_env,
    run_target_proofs_with_channels,
)
from scripts.release_public_evidence import validate_public_evidence_tree
from scripts.release_tool_pins import (
    RUSTC_BINARY_PIN,
    RUSTC_COMMIT_DATE_PIN,
    RUSTC_COMMIT_HASH_PIN,
    RUSTC_LLVM_PIN,
    RUSTC_RELEASE_PIN,
    fixture_lane_tool_evidence,
)
from scripts.stage_speakers_analyze_runtime import (
    DEFAULT_LINK_ROOT as SPEAKERS_ANALYZE_DEFAULT_LINK_ROOT,
)
from scripts.stage_speakers_analyze_runtime import ROOT as SPEAKERS_ANALYZE_STAGE_ROOT
from scripts.stage_speakers_analyze_runtime import TARGETS as SPEAKERS_ANALYZE_TARGETS

Runner = Callable[..., subprocess.CompletedProcess[str]]

CORE_X86_64_MATURIN_ARGS = (
    "--locked --zig --compatibility manylinux2014 --target x86_64-unknown-linux-musl"
)
CORE_AARCH64_MATURIN_ARGS = (
    "--locked --zig --compatibility manylinux2014 --target aarch64-unknown-linux-musl"
)
# The GLIBC_2.34 host-build floor was measured in prep, not anticipated. These
# helper lanes must stay on zig GNU targets so a manylinux_2_27 tag remains true.
# `--auditwheel skip` preserves the data/lib RUNPATH layout, so maturin no
# longer enforces the floor; check_wheel_contents' criterion-14 ELF floor check
# is the guard against a regressed host GNU build being mislabeled.
SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS = (
    "--locked --zig --compatibility manylinux_2_27 --auditwheel skip "
    "--target x86_64-unknown-linux-gnu"
)
SPEAKERS_ANALYZE_AARCH64_MATURIN_ARGS = (
    "--locked --zig --compatibility manylinux_2_27 --auditwheel skip "
    "--target aarch64-unknown-linux-gnu"
)
DESCRIBE_X86_64_MATURIN_ARGS = SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS
DESCRIBE_AARCH64_MATURIN_ARGS = SPEAKERS_ANALYZE_AARCH64_MATURIN_ARGS
ROOT_WORKSPACE_PACKAGE = "solstone"
MODELS_WORKSPACE_PACKAGE = "solstone-journal-models"
CORE_WORKSPACE_PACKAGE = "solstone-core"
SPEAKERS_ANALYZE_WORKSPACE_PACKAGE = "solstone-core-speakers-analyze"
SPEAKERS_ANALYZE_LINK_ROOT_RELATIVE = SPEAKERS_ANALYZE_DEFAULT_LINK_ROOT.relative_to(
    SPEAKERS_ANALYZE_STAGE_ROOT
)
RESERVED_CANDIDATE_DIRNAME = "release-candidate"
RELEASE_CANDIDATE_DISCARD_RETAINED_ENV = "RELEASE_CANDIDATE_DISCARD_RETAINED"
RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV = "RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG"
RETAINED_CANDIDATE_VALID_HEADING = "retained-candidate-valid"
RETAINED_PRE_NVATTEST_CANDIDATE_VALID_HEADING = "retained-pre-nvattest-candidate-valid"

DistPreflightOperation = Literal["cleanup", "inventory"]
DistState = Literal["missing", "directory", "unsafe"]
ReservedState = Literal["unchecked", "absent", "directory", "unsafe"]
TagLookupState = Literal["present", "absent", "undeterminable"]


@dataclass(frozen=True)
class CandidateReport:
    heading: str
    version: str
    release_dir: Path
    evidence_dir: Path
    payload_files: int
    candidate_digest: str
    ledger_sha256: str
    proof_sha256: Mapping[str, str]
    nvattest_sha256: Mapping[str, str]
    bundle_digest: str


@dataclass(frozen=True)
class DryRunPlan:
    models_decision: str
    artifacts: tuple[str, ...]
    tool_evidence: Mapping[str, Mapping[str, str]]
    linux_maturin_args: Mapping[str, str]
    publication_lockout: Mapping[str, bool]


@dataclass(frozen=True)
class TagLookup:
    state: TagLookupState
    commit: str | None
    detail: str | None


@dataclass(frozen=True)
class RetainedCandidatePresence:
    root: Path
    present_paths: tuple[Path, ...]
    absent_paths: tuple[Path, ...]
    failures: tuple[Failure, ...]


@dataclass(frozen=True)
class CandidateServices:
    git_head: Callable[[Path], str]
    git_status: Callable[[Path], str]
    git_tag_commit: Callable[[Path, str], TagLookup]
    core_lock_sha256: Callable[[Path], str]
    clean_outputs: Callable[[Path, str], None]
    build_local_dist: Callable[[Path, bool], None]
    prepare_policy: Callable[[Path, Mapping[str, str]], PolicyRun]
    coordinator_tool_evidence: Callable[[], Mapping[str, Mapping[str, str]]]
    create_source_bundle: Callable[[Path, str, Path], SourceBundle]
    build_host: Callable[[SourceBundle, str, Path], BuildHostResult]
    cleanup_transients: Callable[[Sequence[Path]], None]
    challenge_factory: Callable[[], str]
    materialize_support_wheels: Callable[[Path], Sequence[Path]]
    run_target_proofs: Callable[..., TargetProofPaths]
    transaction_hook: Callable[[str], None]


@dataclass(frozen=True)
class ReservedAccessPolicy:
    mask: int
    access_prose: str
    access_error: str


@dataclass(frozen=True)
class DistPreflightPolicy:
    dist_mask: int
    reserved_access: ReservedAccessPolicy | None
    dist_access_prose: str
    dist_unsafe_error: str
    dist_access_error: str
    reserved_unsafe_error: str
    reserved_check_error: str


@dataclass(frozen=True)
class DistPreflightVerdict:
    dist_state: DistState
    reserved_state: ReservedState
    failures: tuple[Failure, ...]


DIST_PREFLIGHT_POLICIES: Mapping[DistPreflightOperation, DistPreflightPolicy] = {
    "cleanup": DistPreflightPolicy(
        dist_mask=os.R_OK | os.W_OK | os.X_OK,
        reserved_access=ReservedAccessPolicy(
            mask=os.W_OK | os.X_OK,
            access_prose="write/search access",
            access_error=(
                "fresh release cleanup cannot use dist/release-candidate reserved "
                "parent with denied access"
            ),
        ),
        dist_access_prose="read/write/search access",
        dist_unsafe_error="fresh release cleanup requires dist/ to be an owned directory",
        dist_access_error="fresh release cleanup cannot use dist/ with denied access",
        reserved_unsafe_error=(
            "fresh release cleanup found unsafe dist/release-candidate reserved parent"
        ),
        reserved_check_error=(
            "fresh release cleanup could not inspect dist/release-candidate reserved parent"
        ),
    ),
    "inventory": DistPreflightPolicy(
        dist_mask=os.R_OK | os.X_OK,
        reserved_access=None,
        dist_access_prose="read/search access",
        dist_unsafe_error=(
            "local release build inventory requires dist/ to be an owned directory"
        ),
        dist_access_error=(
            "local release build inventory cannot use dist/ with denied access"
        ),
        reserved_unsafe_error=(
            "local release build inventory found unsafe "
            "dist/release-candidate reserved parent"
        ),
        reserved_check_error=(
            "local release build inventory could not inspect "
            "dist/release-candidate reserved parent"
        ),
    ),
}


class DriverError(RuntimeError):
    def __init__(self, failures: Sequence[Failure]) -> None:
        self.failures = tuple(failures)
        super().__init__("; ".join(failure.error for failure in self.failures))


def _failure(error: str, *, expected: str, actual: str, repair: str) -> Failure:
    return Failure(error=error, expected=expected, actual=actual, repair=repair)


def _extract_failure_records(exc: BaseException) -> tuple[Any, ...]:
    failures = getattr(exc, "failures", ())
    if isinstance(failures, (str, bytes)) or not isinstance(failures, Sequence):
        return ()
    records = tuple(failures)
    if not records:
        return ()
    fields = ("error", "expected", "actual", "repair")
    if not all(
        all(isinstance(getattr(record, field, None), str) for field in fields)
        for record in records
    ):
        return ()
    return records


def _run_stdout(
    runner: Runner,
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str] | None = None,
) -> str:
    result = runner(
        list(argv),
        cwd=cwd,
        env=dict(env) if env is not None else None,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise DriverError(
            [
                _failure(
                    "release driver command failed",
                    expected="exit 0",
                    actual=result.stderr.strip()
                    or result.stdout.strip()
                    or f"exit {result.returncode}",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return result.stdout.strip()


def _project_version(root: Path) -> str:
    data = tomllib.loads((root / "pyproject.toml").read_text(encoding="utf-8"))
    return str(data["project"]["version"])


def _local_models_package_version(root: Path) -> str:
    data = tomllib.loads(
        (root / "packages" / "solstone-journal-models" / "pyproject.toml").read_text(
            encoding="utf-8"
        )
    )
    return str(data["project"]["version"])


def _default_git_head(root: Path) -> str:
    return _run_stdout(subprocess.run, ["git", "rev-parse", "HEAD"], cwd=root)


def _default_git_status(root: Path) -> str:
    return _run_stdout(
        subprocess.run,
        ["git", "status", "--porcelain", "--untracked-files=normal"],
        cwd=root,
    )


def _default_git_tag_commit(root: Path, version: str) -> TagLookup:
    result = subprocess.run(
        [
            "git",
            "rev-parse",
            "--verify",
            "--quiet",
            f"refs/tags/v{version}^{{}}",
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode == 0:
        return TagLookup(state="present", commit=result.stdout.strip(), detail=None)
    if result.returncode == 1:
        return TagLookup(state="absent", commit=None, detail=None)
    return TagLookup(
        state="undeterminable",
        commit=None,
        detail=f"git rev-parse exit {result.returncode}",
    )


def _default_core_lock_sha256(root: Path) -> str:
    digest, _bytes = file_sha256_size(root / "core" / "Cargo.lock")
    return digest


def _retained_relative(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def _format_retained_paths(root: Path, paths: Sequence[Path]) -> str:
    if not paths:
        return "<none>"
    return ", ".join(_retained_relative(root, path) for path in paths)


def _undeterminable_retained_state_failure(version: str, actual: str) -> Failure:
    return _failure(
        "retained release evidence state is undeterminable",
        expected=(
            "retained release-candidate payload/evidence and published tag state "
            "can be determined before cleanup"
        ),
        actual=actual,
        repair=(
            "fix the unreadable retained release state; "
            f"{RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}={version} and "
            f"{RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}={version}+v{version} "
            "do not apply to undeterminable state"
        ),
    )


def _retained_candidate_presence(root: Path, version: str) -> RetainedCandidatePresence:
    release_dir = root / "dist" / RESERVED_CANDIDATE_DIRNAME / version
    evidence_dir = root / "target" / "release-evidence" / version
    present: list[Path] = []
    absent: list[Path] = []
    failures: list[Failure] = []
    for path in (release_dir, evidence_dir):
        try:
            path.lstat()
        except FileNotFoundError:
            absent.append(path)
        except OSError as exc:
            failures.append(
                _undeterminable_retained_state_failure(
                    version,
                    (
                        "retained path check could not inspect "
                        f"{_retained_relative(root, path)}: {type(exc).__name__}"
                    ),
                )
            )
        else:
            present.append(path)
    return RetainedCandidatePresence(
        root=root,
        present_paths=tuple(present),
        absent_paths=tuple(absent),
        failures=tuple(failures),
    )


def _authorization_version(value: str) -> str:
    return value.split("+", 1)[0]


def _authorization_mismatch_failure(
    *,
    variable: str,
    value: str,
    stated_version: str,
    version: str,
) -> Failure:
    if variable == RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV:
        repair = (
            f"set {RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}={version}+v{version} "
            f"or unset {RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}"
        )
    else:
        repair = (
            f"set {RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}={version} "
            f"or unset {RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}"
        )
    stated_version_display = stated_version or "<missing>"
    return _failure(
        "release candidate discard authorization names a different version",
        expected=f"{variable}=<version> matching working-tree version {version}",
        actual=(
            f"{variable}={value}; authorization version {stated_version_display}; "
            f"working-tree version {version}"
        ),
        repair=repair,
    )


def _authorization_mismatch_failures(
    env: Mapping[str, str], version: str
) -> list[Failure]:
    failures: list[Failure] = []
    for variable in (
        RELEASE_CANDIDATE_DISCARD_RETAINED_ENV,
        RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV,
    ):
        value = env.get(variable, "")
        if not value:
            continue
        stated_version = _authorization_version(value)
        if stated_version != version:
            failures.append(
                _authorization_mismatch_failure(
                    variable=variable,
                    value=value,
                    stated_version=stated_version,
                    version=version,
                )
            )
    return failures


def _retained_candidate_authorization_failures(
    presence: RetainedCandidatePresence,
    tag_lookup: TagLookup,
    env: Mapping[str, str],
    version: str,
) -> list[Failure]:
    state = tag_lookup.state
    if state == "undeterminable":
        detail = tag_lookup.detail or "git rev-parse exit unknown"
        return [
            _undeterminable_retained_state_failure(
                version,
                f"tag lookup for v{version} was undeterminable: {detail}",
            )
        ]
    failures = _authorization_mismatch_failures(env, version)
    if failures:
        return failures
    hard_value = f"{version}+v{version}"
    hard_authorized = (
        env.get(RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV, "") == hard_value
    )
    if state == "present":
        if hard_authorized:
            return []
        commit = tag_lookup.commit or "<missing>"
        return [
            _failure(
                "published retained release evidence would be discarded",
                expected=(
                    "no retained release-candidate payload/evidence for published tag, "
                    "or RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG=<version>+<tag>"
                ),
                actual=(
                    f"version {version}; tag v{version} -> {commit}; "
                    "present retained paths: "
                    f"{_format_retained_paths(presence.root, presence.present_paths)}"
                ),
                repair=(
                    "set "
                    f"{RELEASE_CANDIDATE_DISCARD_PUBLISHED_TAG_ENV}={version}+v{version} "
                    f"to discard retained payload/evidence for published tag v{version}"
                ),
            )
        ]
    if state != "absent":
        return [
            _undeterminable_retained_state_failure(
                version,
                f"tag lookup for v{version} returned unrecognized state {state!r}",
            )
        ]
    soft_authorized = env.get(RELEASE_CANDIDATE_DISCARD_RETAINED_ENV, "") == version
    if soft_authorized or hard_authorized:
        return []
    return [
        _failure(
            "retained release evidence would be discarded",
            expected=(
                "no retained release-candidate payload/evidence, or "
                "RELEASE_CANDIDATE_DISCARD_RETAINED=<version>"
            ),
            actual=(
                f"working-tree version {version}; colliding retained paths: "
                f"{_format_retained_paths(presence.root, presence.present_paths)}; "
                "absent retained paths: "
                f"{_format_retained_paths(presence.root, presence.absent_paths)}"
            ),
            repair=(
                f"set {RELEASE_CANDIDATE_DISCARD_RETAINED_ENV}={version} "
                "to discard retained payload/evidence for this unpublished candidate"
            ),
        )
    ]


def _is_missing(path: Path) -> bool:
    try:
        path.lstat()
    except FileNotFoundError:
        return True
    return False


def _remove_owned_path(path: Path, *, label: str) -> list[Failure]:
    try:
        entry = path.lstat()
    except FileNotFoundError:
        return []
    if stat.S_ISLNK(entry.st_mode):
        return [
            _failure(
                "fresh release cleanup refused symlink residue",
                expected=f"{label} owned non-symlink path",
                actual=path.name,
                repair="bash scripts/release.sh --candidate",
            )
        ]
    try:
        if stat.S_ISDIR(entry.st_mode):
            shutil.rmtree(path)
        elif stat.S_ISREG(entry.st_mode):
            path.unlink()
        else:
            return [
                _failure(
                    "fresh release cleanup refused non-regular residue",
                    expected=f"{label} owned directory or regular file",
                    actual=path.name,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
    except OSError as exc:
        return [
            _failure(
                "fresh release cleanup could not remove owned residue",
                expected=f"{label} removed",
                actual=type(exc).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        ]
    return []


def _remove_owned_relative(root: Path, relative: Path) -> list[Failure]:
    current = root
    for part in relative.parts[:-1]:
        current = current / part
        try:
            entry = current.lstat()
        except FileNotFoundError:
            return []
        if stat.S_ISLNK(entry.st_mode) or not stat.S_ISDIR(entry.st_mode):
            return [
                _failure(
                    "fresh release cleanup refused unsafe parent",
                    expected=f"{relative.as_posix()} parent is an owned directory",
                    actual=part,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
    return _remove_owned_path(root / relative, label=relative.as_posix())


def _owned_glob(
    parent: Path, pattern: str, *, label: str
) -> tuple[list[Path], list[Failure]]:
    try:
        entry = parent.lstat()
    except FileNotFoundError:
        return [], []
    if stat.S_ISLNK(entry.st_mode) or not stat.S_ISDIR(entry.st_mode):
        return [], [
            _failure(
                "fresh release cleanup refused unsafe parent",
                expected=f"{label} owned non-symlink directory",
                actual=parent.name,
                repair="bash scripts/release.sh --candidate",
            )
        ]
    return list(parent.glob(pattern)), []


def _path_kind(entry: os.stat_result) -> str:
    if stat.S_ISLNK(entry.st_mode):
        return "symlink"
    if stat.S_ISREG(entry.st_mode):
        return "regular file"
    if stat.S_ISDIR(entry.st_mode):
        return "directory"
    return "special file"


def _dist_expected(policy: DistPreflightPolicy) -> str:
    return f"dist/ owned non-symlink directory with {policy.dist_access_prose}"


def _reserved_expected(policy: DistPreflightPolicy) -> str:
    expected = "dist/release-candidate absent or an owned non-symlink directory"
    if policy.reserved_access is not None:
        expected = f"{expected} with {policy.reserved_access.access_prose}"
    return expected


def _dist_preflight(
    dist_dir: Path, *, operation: DistPreflightOperation
) -> DistPreflightVerdict:
    """Check dist and its retained-candidate child before release mutations."""
    policy = DIST_PREFLIGHT_POLICIES[operation]
    try:
        dist_entry = dist_dir.lstat()
    except FileNotFoundError:
        return DistPreflightVerdict(
            dist_state="missing", reserved_state="unchecked", failures=()
        )
    if stat.S_ISLNK(dist_entry.st_mode) or not stat.S_ISDIR(dist_entry.st_mode):
        return DistPreflightVerdict(
            dist_state="unsafe",
            reserved_state="unchecked",
            failures=(
                _failure(
                    policy.dist_unsafe_error,
                    expected=_dist_expected(policy),
                    actual=f"dist/ is {_path_kind(dist_entry)}",
                    repair="bash scripts/release.sh --candidate",
                ),
            ),
        )
    if not os.access(dist_dir, policy.dist_mask):
        return DistPreflightVerdict(
            dist_state="directory",
            reserved_state="unchecked",
            failures=(
                _failure(
                    policy.dist_access_error,
                    expected=_dist_expected(policy),
                    actual=f"dist/ lacks {policy.dist_access_prose}",
                    repair="bash scripts/release.sh --candidate",
                ),
            ),
        )
    path = dist_dir / RESERVED_CANDIDATE_DIRNAME
    try:
        entry = path.lstat()
    except FileNotFoundError:
        return DistPreflightVerdict(
            dist_state="directory", reserved_state="absent", failures=()
        )
    except OSError as exc:
        return DistPreflightVerdict(
            dist_state="directory",
            reserved_state="unsafe",
            failures=(
                _failure(
                    policy.reserved_check_error,
                    expected=_reserved_expected(policy),
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                ),
            ),
        )
    if stat.S_ISLNK(entry.st_mode):
        actual = "dist/release-candidate is symlink"
    elif stat.S_ISREG(entry.st_mode):
        actual = "dist/release-candidate is regular file"
    elif stat.S_ISDIR(entry.st_mode):
        if policy.reserved_access is None:
            return DistPreflightVerdict(
                dist_state="directory", reserved_state="directory", failures=()
            )
        if os.access(path, policy.reserved_access.mask):
            return DistPreflightVerdict(
                dist_state="directory", reserved_state="directory", failures=()
            )
        return DistPreflightVerdict(
            dist_state="directory",
            reserved_state="unsafe",
            failures=(
                _failure(
                    policy.reserved_access.access_error,
                    expected=_reserved_expected(policy),
                    actual=(
                        "dist/release-candidate lacks "
                        f"{policy.reserved_access.access_prose}"
                    ),
                    repair="bash scripts/release.sh --candidate",
                ),
            ),
        )
    else:
        actual = "dist/release-candidate is special file"
    return DistPreflightVerdict(
        dist_state="directory",
        reserved_state="unsafe",
        failures=(
            _failure(
                policy.reserved_unsafe_error,
                expected=_reserved_expected(policy),
                actual=actual,
                repair="bash scripts/release.sh --candidate",
            ),
        ),
    )


def _clean_raw_dist_outputs(root: Path, verdict: DistPreflightVerdict) -> list[Failure]:
    if verdict.dist_state != "directory":
        return []
    dist = root / "dist"
    failures: list[Failure] = []
    try:
        children = sorted(dist.iterdir(), key=lambda path: path.name)
    except OSError as exc:
        return [
            _failure(
                "fresh release cleanup could not inspect raw dist outputs",
                expected="readable dist directory",
                actual=type(exc).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        ]
    for child in children:
        if (
            verdict.reserved_state == "directory"
            and child.name == RESERVED_CANDIDATE_DIRNAME
        ):
            continue
        failures.extend(_remove_owned_path(child, label=f"dist/{child.name}"))
    return failures


def _payload_transient_paths(root: Path, version: str) -> tuple[Path, ...]:
    ready_path = root / "dist" / RESERVED_CANDIDATE_DIRNAME / version
    payload_staging = ready_path.parent / f"{version}.payload-staging"
    return (
        ready_path,
        payload_staging,
        payload_staging.parent / f"{payload_staging.name}.staging",
        payload_staging.parent / f"{payload_staging.name}.quarantine",
    )


def _zig_cache_root(root: Path) -> Path:
    return root / "target" / "release-zig-cache"


def _source_bundle_staging_path(root: Path, version: str) -> Path:
    return root / "target" / "release-transfer" / f".{version}.source.bundle"


def _default_clean_outputs(root: Path, version: str) -> None:
    verdict = _dist_preflight(root / "dist", operation="cleanup")
    if verdict.failures:
        raise DriverError(verdict.failures)
    failures: list[Failure] = []
    failures.extend(_remove_owned_path(root / "build", label="build"))
    failures.extend(_clean_raw_dist_outputs(root, verdict))
    root_egg_infos, root_glob_failures = _owned_glob(
        root, "*.egg-info", label="repository root"
    )
    failures.extend(root_glob_failures)
    for egg_info in root_egg_infos:
        failures.extend(_remove_owned_path(egg_info, label="root egg-info"))
    for package in (
        "solstone-journal",
        "solstone-journal-cuda",
        "solstone-journal-models",
        SPEAKERS_ANALYZE_WORKSPACE_PACKAGE,
    ):
        package_dir = root / "packages" / package
        egg_infos, glob_failures = _owned_glob(
            package_dir, "*.egg-info", label=f"{package} package directory"
        )
        failures.extend(glob_failures)
        for egg_info in egg_infos:
            failures.extend(_remove_owned_path(egg_info, label=f"{package} egg-info"))
    for relative in (
        Path("target") / "release-evidence" / version,
        Path("target") / "release-evidence" / f"{version}.staging",
        Path("target") / "release-transfer" / version,
        _source_bundle_staging_path(root, version).relative_to(root),
        _zig_cache_root(root).relative_to(root),
        Path("packages") / SPEAKERS_ANALYZE_WORKSPACE_PACKAGE / "wheel-data",
    ):
        failures.extend(_remove_owned_relative(root, relative))
    if verdict.reserved_state == "directory":
        for path in _payload_transient_paths(root, version):
            failures.extend(_remove_owned_path(path, label=path.name))
    transfer_parent = root / "target" / "release-transfer"
    request_siblings, request_failures = _owned_glob(
        transfer_parent,
        f".{version}.request-*",
        label="release transfer directory",
    )
    failures.extend(request_failures)
    for path in request_siblings:
        failures.extend(_remove_owned_path(path, label="release transfer request"))
    if failures:
        raise DriverError(failures)


def _linux_maturin_tokens(target: str) -> tuple[str, ...]:
    return (
        "--locked",
        "--zig",
        "--compatibility",
        "manylinux2014",
        "--target",
        target,
    )


def validate_linux_maturin_args(args: str, *, target: str) -> list[Failure]:
    try:
        tokens = tuple(shlex.split(args))
    except ValueError as exc:
        return [
            _failure(
                "Linux maturin arguments are not parseable",
                expected="exact Linux maturin token contract",
                actual=str(exc),
                repair="bash scripts/release.sh --candidate",
            )
        ]
    expected = _linux_maturin_tokens(target)
    if tokens != expected:
        return [
            _failure(
                "Linux maturin arguments do not match release contract",
                expected=" ".join(expected),
                actual=" ".join(tokens),
                repair="bash scripts/release.sh --candidate",
            )
        ]
    return []


def _speakers_analyze_linux_maturin_tokens(target: str) -> tuple[str, ...]:
    return (
        "--locked",
        "--zig",
        "--compatibility",
        "manylinux_2_27",
        "--auditwheel",
        "skip",
        "--target",
        target,
    )


def validate_speakers_analyze_linux_maturin_args(
    args: str, *, target: str
) -> list[Failure]:
    try:
        tokens = tuple(shlex.split(args))
    except ValueError as exc:
        return [
            _failure(
                "speakers analyze Linux maturin arguments are not parseable",
                expected="exact speakers analyze Linux maturin token contract",
                actual=str(exc),
                repair="bash scripts/release.sh --candidate",
            )
        ]
    expected = _speakers_analyze_linux_maturin_tokens(target)
    if tokens != expected:
        return [
            _failure(
                "speakers analyze Linux maturin arguments do not match release contract",
                expected=" ".join(expected),
                actual=" ".join(tokens),
                repair="bash scripts/release.sh --candidate",
            )
        ]
    return []


def _create_zig_cache_dirs(root: Path) -> tuple[Path, Path]:
    cache_root = _zig_cache_root(root)
    cache_root_label = cache_root.relative_to(root).as_posix()
    global_cache = cache_root / "zig-global"
    local_cache = cache_root / "zig-local"
    try:
        global_cache.mkdir(parents=True, exist_ok=True)
        local_cache.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise DriverError(
            [
                _failure(
                    "release Zig cache directory could not be created",
                    expected=f"writable Zig cache directories under {cache_root_label}",
                    actual=f"{type(exc).__name__}: {exc}",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from None
    return global_cache.resolve(), local_cache.resolve()


def _scrubbed_build_env(
    root: Path, maturin_args: str, ort_target: str | None
) -> dict[str, str]:
    zig_global_cache, zig_local_cache = _create_zig_cache_dirs(root)
    # Local release builds use a narrow env, not a fully synthetic HOME. Keys:
    # - MATURIN_PEP517_ARGS: gives the PEP517 backend the locked Linux args.
    # - ORT_LIB_PATH: gives only speakers-analyze helper wheel builds the staged
    #   target-specific ONNX Runtime link directory for the target being built.
    # - ORT_PREFER_DYNAMIC_LINK: paired with ORT_LIB_PATH so the helper links to
    #   the staged shared library. It is target-scoped because core/musl builds
    #   do not bundle that runtime and must stay ORT-free.
    # - PATH: the only ambient value copied, solely for tool discovery.
    # - PYTHONNOUSERSITE: keeps Python from importing user-site packages.
    # - ZIG_GLOBAL_CACHE_DIR: required by Zig 0.16.0 without HOME/XDG appdata.
    # - ZIG_LOCAL_CACHE_DIR: keeps Zig's local build cache out of cwd defaults.
    #
    # Tool disposition is intentional. Zig is hermetic because it is the only
    # tool here with no passwd fallback and the only one that fails loudly.
    # uv stays passwd-warm: with no HOME it exits 0 and resolves the operator's
    # uv cache directory. maturin owns no cache and is cwd-relative. cargo and
    # rustc stay passwd-warm through rustup shims that reach the operator's
    # rustup and cargo home directories. python3 stays passwd-warm: Path.home()
    # resolves to the operator's home. Making uv/cargo transaction-local forced
    # network-dependent re-resolution in probes: crates.io index plus 77 .crate
    # downloads and a 113M cargo cache per candidate, weakening --locked offline
    # determinism.
    env = {
        "MATURIN_PEP517_ARGS": maturin_args,
        "PATH": os.environ.get("PATH", ""),
        "PYTHONNOUSERSITE": "1",
        "ZIG_GLOBAL_CACHE_DIR": str(zig_global_cache),
        "ZIG_LOCAL_CACHE_DIR": str(zig_local_cache),
    }
    if ort_target is not None:
        if ort_target not in SPEAKERS_ANALYZE_TARGETS:
            valid = ", ".join(sorted(SPEAKERS_ANALYZE_TARGETS))
            raise ValueError(
                f"unknown speakers-analyze ORT target {ort_target!r}; "
                f"expected one of: {valid}"
            )
        spec = SPEAKERS_ANALYZE_TARGETS[ort_target]
        env["ORT_LIB_PATH"] = str(
            (root / SPEAKERS_ANALYZE_LINK_ROOT_RELATIVE / spec.key).resolve()
        )
        env["ORT_PREFER_DYNAMIC_LINK"] = "true"
    return env


def _expected_local_build_packages(*, include_models: bool) -> tuple[str, ...]:
    inventory = load_release_package_inventory(ROOT)
    packages = {inventory.root_distribution, *inventory.workspace_distributions}
    if not include_models:
        packages -= {MODELS_WORKSPACE_PACKAGE}
    return tuple(sorted(packages))


def _expected_local_build_commands(
    *, include_models: bool, version: str
) -> tuple[tuple[tuple[str, ...], str, str | None], ...]:
    render_check = (("python3", "scripts/render_packaging.py", "--check"), "", None)
    inventory = load_release_package_inventory(ROOT)
    native_distributions = {
        package.distribution for package in inventory.native_packages
    }
    package_builds = tuple(
        (("uv", "build", "--package", package), "", None)
        for package in _expected_local_build_packages(include_models=include_models)
        if package not in native_distributions
    )
    core_sdist = (
        ("uv", "build", "--package", CORE_WORKSPACE_PACKAGE, "--sdist"),
        "",
        None,
    )
    core_sdist_path = f"dist/solstone_core-{version}.tar.gz"
    x86_64_core = (
        ("uv", "build", core_sdist_path, "--wheel", "--out-dir", "dist"),
        CORE_X86_64_MATURIN_ARGS,
        None,
    )
    aarch64_core = (
        ("uv", "build", core_sdist_path, "--wheel", "--out-dir", "dist"),
        CORE_AARCH64_MATURIN_ARGS,
        None,
    )
    native_builds: list[tuple[tuple[str, ...], str, str | None]] = []
    for package in inventory.native_packages:
        if package.distribution == CORE_WORKSPACE_PACKAGE:
            continue
        if package.target_family == "core":
            build_args = (
                (CORE_X86_64_MATURIN_ARGS, None),
                (CORE_AARCH64_MATURIN_ARGS, None),
            )
        elif package.target_family == "describe":
            build_args = (
                (DESCRIBE_X86_64_MATURIN_ARGS, None),
                (DESCRIBE_AARCH64_MATURIN_ARGS, None),
            )
        else:
            build_args = (
                (SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS, "linux-x86_64"),
                (SPEAKERS_ANALYZE_AARCH64_MATURIN_ARGS, "linux-aarch64"),
            )
        for maturin_args, ort_target in build_args:
            if ort_target is not None:
                native_builds.append(
                    (
                        (
                            "python3",
                            "scripts/stage_speakers_analyze_runtime.py",
                            "--target",
                            ort_target,
                        ),
                        "",
                        None,
                    )
                )
            native_builds.append(
                (
                    ("uv", "build", "--package", package.distribution, "--wheel"),
                    maturin_args,
                    ort_target,
                )
            )
    return (
        render_check,
        *package_builds,
        core_sdist,
        x86_64_core,
        aarch64_core,
        *native_builds,
    )


def _default_build_local_dist(
    root: Path,
    include_models: bool,
    *,
    runner: Runner = subprocess.run,
) -> None:
    linux_contracts = (
        (CORE_X86_64_MATURIN_ARGS, "x86_64-unknown-linux-musl"),
        (CORE_AARCH64_MATURIN_ARGS, "aarch64-unknown-linux-musl"),
    )
    speakers_analyze_contracts = (
        (SPEAKERS_ANALYZE_X86_64_MATURIN_ARGS, "x86_64-unknown-linux-gnu"),
        (SPEAKERS_ANALYZE_AARCH64_MATURIN_ARGS, "aarch64-unknown-linux-gnu"),
    )
    describe_contracts = (
        (DESCRIBE_X86_64_MATURIN_ARGS, "x86_64-unknown-linux-gnu"),
        (DESCRIBE_AARCH64_MATURIN_ARGS, "aarch64-unknown-linux-gnu"),
    )
    failures: list[Failure] = []
    for args, target in linux_contracts:
        failures.extend(validate_linux_maturin_args(args, target=target))
    for args, target in speakers_analyze_contracts:
        failures.extend(
            validate_speakers_analyze_linux_maturin_args(args, target=target)
        )
    for args, target in describe_contracts:
        failures.extend(
            validate_speakers_analyze_linux_maturin_args(args, target=target)
        )
    if failures:
        raise DriverError(failures)
    version = _project_version(root)
    core_sdist_argv = (
        "uv",
        "build",
        "--package",
        CORE_WORKSPACE_PACKAGE,
        "--sdist",
    )
    core_sdist = root / "dist" / f"solstone_core-{version}.tar.gz"
    for argv, maturin_args, ort_target in _expected_local_build_commands(
        include_models=include_models,
        version=version,
    ):
        env = _scrubbed_build_env(root, maturin_args, ort_target)
        _run_stdout(
            runner,
            list(argv),
            cwd=root,
            env=env,
        )
        if argv == core_sdist_argv:
            try:
                normalize_core_sdist_workspace_lock(root, core_sdist)
            except SdistLockError as exc:
                raise DriverError(
                    [
                        _failure(
                            "local core sdist workspace lock normalization failed",
                            expected=(
                                "Cargo.lock aligned with Maturin's pruned sdist workspace"
                            ),
                            actual=str(exc),
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                ) from None
    # uv build auto-creates dist/.gitignore with ignore-all content "*".
    # Strip exactly that artifact before the local-dist inventory gate.
    # Leave anything else in dist for the inventory gate to reject.
    _remove_uv_dist_gitignore(root / "dist")
    _validate_local_dist_inventory(root / "dist", include_models=include_models)
    cleanup_failures = _remove_owned_relative(
        root,
        Path("packages") / SPEAKERS_ANALYZE_WORKSPACE_PACKAGE / "wheel-data",
    )
    if cleanup_failures:
        raise DriverError(cleanup_failures)


def _remove_uv_dist_gitignore(dist_dir: Path) -> None:
    path = dist_dir / ".gitignore"
    try:
        entry = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISREG(entry.st_mode):
        return
    try:
        content = path.read_bytes()
    except OSError as exc:
        raise DriverError(
            [
                _failure(
                    "local release build could not inspect uv dist/.gitignore",
                    expected="readable uv build dist/.gitignore marker",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from exc
    if content not in {b"*", b"*\n"}:
        return
    try:
        path.unlink()
    except OSError as exc:
        raise DriverError(
            [
                _failure(
                    "local release build could not remove uv dist/.gitignore",
                    expected="uv build dist/.gitignore marker removed",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from exc


def _default_prepare_policy(root: Path, env: Mapping[str, str]) -> PolicyRun:
    return prepare_policy_run(
        root,
        advisory_source_id=env["RELEASE_ADVISORY_SOURCE_NAME"],
        db_urls=(env["RELEASE_ADVISORY_DB_URL"],),
        db_root=Path(env["RELEASE_ADVISORY_DB_ROOT"]),
    )


def _default_create_source_bundle(
    root: Path, commit: str, output_path: Path
) -> SourceBundle:
    return create_source_bundle(root, expected_commit=commit, output_path=output_path)


def _default_build_host_from_env(
    env: Mapping[str, str],
) -> Callable[[SourceBundle, str, Path], BuildHostResult]:
    channel = ExternalBuildHostChannel.from_env(env)

    def build_host(
        source_bundle: SourceBundle, commit: str, output_dir: Path
    ) -> BuildHostResult:
        return channel.build_macos(
            source_bundle=source_bundle,
            expected_commit=commit,
            output_dir=output_dir,
        )

    return build_host


def _default_coordinator_tool_evidence() -> dict[str, dict[str, str]]:
    return {
        lane: collect_lane_tool_evidence(lane)
        for lane in ("source", "linux-x86_64-musl", "linux-aarch64-musl")
    }


def _default_cleanup_transients(paths: Sequence[Path]) -> None:
    for path in paths:
        try:
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists() or path.is_symlink():
                path.unlink()
        except OSError as exc:
            raise DriverError(
                [
                    _failure(
                        "release transient cleanup failed",
                        expected="owned release transients removed",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            ) from None


def _default_materialize_support_wheels(destination: Path) -> tuple[Path, ...]:
    try:
        root = destination.parents[3]
    except IndexError:
        raise DriverError(
            [
                _failure(
                    "nvattest support materialization failed",
                    expected="evidence support directory under release checkout",
                    actual=destination.name,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from None
    try:
        entries = read_support_lock_entries(root / "uv.lock")
    except SupportLockError as exc:
        raise DriverError(_driver_failures_from_support_error(exc)) from None
    destination.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    for entry in entries:
        output_path = destination / entry.filename
        temp_path = output_path.with_name(f".{output_path.name}.tmp")
        try:
            with urllib.request.urlopen(entry.url, timeout=30) as response:
                temp_path.write_bytes(response.read())
            os.rename(temp_path, output_path)
        finally:
            temp_path.unlink(missing_ok=True)
        paths.append(output_path)
    return tuple(paths)


def default_services(env: Mapping[str, str] | None = None) -> CandidateServices:
    build_host = (
        _default_build_host_from_env(env) if env is not None else _missing_build_host
    )
    proof_runner = (
        _default_proof_hosts_from_env(env) if env is not None else _missing_proof_host
    )
    return CandidateServices(
        git_head=_default_git_head,
        git_status=_default_git_status,
        git_tag_commit=_default_git_tag_commit,
        core_lock_sha256=_default_core_lock_sha256,
        clean_outputs=_default_clean_outputs,
        build_local_dist=_default_build_local_dist,
        prepare_policy=_default_prepare_policy,
        coordinator_tool_evidence=_default_coordinator_tool_evidence,
        create_source_bundle=_default_create_source_bundle,
        build_host=build_host,
        cleanup_transients=_default_cleanup_transients,
        challenge_factory=lambda: secrets.token_hex(32),
        materialize_support_wheels=_default_materialize_support_wheels,
        run_target_proofs=proof_runner,
        transaction_hook=lambda _point: None,
    )


def _missing_build_host(
    _source_bundle: SourceBundle, _commit: str, _output_dir: Path
) -> BuildHostResult:
    raise DriverError(
        [
            _failure(
                "build-host service is not injected",
                expected="external build-host channel",
                actual="<missing>",
                repair="bash scripts/release.sh --candidate",
            )
        ]
    )


def _default_proof_hosts_from_env(
    env: Mapping[str, str],
) -> Callable[..., TargetProofPaths]:
    try:
        channels = proof_channels_from_env(env)
    except ProofHostError as exc:
        raise DriverError(exc.failures) from None

    def proof_runner(**kwargs: Any) -> TargetProofPaths:
        try:
            return run_target_proofs_with_channels(channels, **kwargs)
        except ProofHostError as exc:
            raise DriverError(exc.failures) from None

    return proof_runner


def _missing_proof_host(**_kwargs: Any) -> TargetProofPaths:
    raise DriverError(
        [
            _failure(
                "proof-host service is not injected",
                expected="configured proof-host channels for all targets",
                actual="<missing>",
                repair="bash scripts/release.sh --candidate",
            )
        ]
    )


def _expected_commit(env: Mapping[str, str]) -> str:
    value = env.get("EXPECTED_RELEASE_COMMIT", "")
    if not SOURCE_COMMIT_RE.fullmatch(value):
        raise DriverError(
            [
                _failure(
                    "EXPECTED_RELEASE_COMMIT is invalid",
                    expected="40 or 64 lowercase hexadecimal characters",
                    actual=value or "<missing>",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return value


def _models_include(env: Mapping[str, str]) -> bool:
    value = env.get("RELEASE_MODEL_PACKAGES", "")
    if value not in {"include", "exclude"}:
        raise DriverError(
            [
                _failure(
                    "release model package decision is invalid",
                    expected="RELEASE_MODEL_PACKAGES=include or exclude",
                    actual=value or "<missing>",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return value == "include"


def _assert_clean_identity(
    root: Path,
    *,
    expected_commit: str,
    expected_lock_sha256: str,
    services: CandidateServices,
) -> None:
    head = services.git_head(root)
    if head != expected_commit:
        raise DriverError(
            [
                _failure(
                    "release source commit does not match EXPECTED_RELEASE_COMMIT",
                    expected=expected_commit,
                    actual=head,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    status = services.git_status(root)
    if status:
        raise DriverError(
            [
                _failure(
                    "release source tree is not clean",
                    expected="empty git status",
                    actual=status,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    actual_lock = services.core_lock_sha256(root)
    if actual_lock != expected_lock_sha256:
        raise DriverError(
            [
                _failure(
                    "core lock hash changed before finalization",
                    expected=expected_lock_sha256,
                    actual=actual_lock,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )


def _macos_wheel_role(path: Path) -> str | None:
    return {name: role for role, name in _expected_macos_wheels_by_role().items()}.get(
        path.name
    )


def _expected_macos_wheels_by_role() -> dict[str, str]:
    inventory = load_release_package_inventory(ROOT)
    expected_wheels = expected_package_names(include_models=False)
    expected_root = next(
        item
        for item in expected_wheels
        if item.startswith("solstone-") and "macosx_14_0_arm64" in item
    )
    by_role = {"root": expected_root}
    for package in inventory.macos_native_packages:
        prefix = f"{normalized_distribution(package.distribution)}-"
        matches = [
            item
            for item in expected_wheels
            if item.startswith(prefix) and "macosx_14_0_arm64" in item
        ]
        if len(matches) != 1:
            raise RuntimeError(
                f"release artifacts did not include exactly one macOS wheel for "
                f"{package.distribution}"
            )
        by_role[native_role(package)] = matches[0]
    return by_role


def _expected_macos_roles() -> set[str]:
    return set(_expected_macos_wheels_by_role())


def _macos_wheel_names() -> frozenset[str]:
    return frozenset(
        name
        for name in expected_package_names(include_models=False)
        if _macos_wheel_role(Path(name)) is not None
    )


def _expected_local_dist_names(*, include_models: bool) -> frozenset[str]:
    return (
        frozenset(expected_package_names(include_models=include_models))
        - _macos_wheel_names()
    )


def _validate_local_dist_inventory(dist_dir: Path, *, include_models: bool) -> None:
    verdict = _dist_preflight(dist_dir, operation="inventory")
    failures: list[Failure] = []
    if verdict.dist_state == "missing":
        raise DriverError(
            [
                _failure(
                    "local release build did not produce dist",
                    expected="dist directory with local package artifacts",
                    actual="missing",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    if verdict.failures:
        raise DriverError(verdict.failures)
    names: set[str] = set()
    for path in dist_dir.iterdir():
        if (
            verdict.reserved_state == "directory"
            and path.name == RESERVED_CANDIDATE_DIRNAME
        ):
            continue
        child = path.lstat()
        if stat.S_ISREG(child.st_mode):
            names.add(path.name)
        else:
            failures.append(
                _failure(
                    "local release build produced unsafe dist entry",
                    expected="regular package artifact files only",
                    actual=path.name,
                    repair="bash scripts/release.sh --candidate",
                )
            )
    actual = frozenset(names)
    expected = _expected_local_dist_names(include_models=include_models)
    if actual != expected:
        failures.append(
            _failure(
                "local release build artifact inventory does not match models decision",
                expected=", ".join(sorted(expected)),
                actual=", ".join(sorted(actual)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    linux_core_wheels = [
        name
        for name in actual
        if name.startswith("solstone_core-")
        and ("manylinux2014_x86_64" in name or "manylinux2014_aarch64" in name)
    ]
    if len(linux_core_wheels) != 2:
        failures.append(
            _failure(
                "local release build did not produce both Linux musl core wheels",
                expected="x86_64 and aarch64 musl solstone-core wheels",
                actual=", ".join(sorted(linux_core_wheels)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if failures:
        raise DriverError(failures)


def _validate_candidate_wheel_contents(
    release_dir: Path, *, models_decision: str
) -> None:
    wheel_models_decision = "publish" if models_decision == "include" else "skip"
    errors = check_dist(
        release_dir,
        EXPECTED_MODEL_SHA256,
        MAX_BASE_WHEEL_BYTES,
        required_core_platforms=(),
        release_scope="all-hosts",
        models_decision=wheel_models_decision,
    )
    if errors:
        raise DriverError(
            [
                _failure(
                    "release candidate wheel content check failed",
                    expected="candidate wheels matching platform content policy",
                    actual=error,
                    repair=(
                        "rebuild the candidate with bash scripts/release.sh --candidate "
                        "after fixing the reported wheel content"
                    ),
                )
                for error in errors
            ]
        )


def _native_record_role(path: Path) -> str | None:
    return {
        macos_native_record_name(role): role for role in _expected_macos_roles()
    }.get(path.name)


def _native_record_payloads(
    host_result: BuildHostResult,
    *,
    source_commit: str,
    core_lock_sha256: str,
) -> list[dict[str, Any]]:
    failures: list[Failure] = []
    expected_roles = _expected_macos_roles()
    wheel_by_role: dict[str, Path] = {}
    for wheel in host_result.macos_wheels:
        role = _macos_wheel_role(wheel)
        if role is None:
            failures.append(
                _failure(
                    "build-host macOS wheel role is invalid",
                    expected=", ".join(sorted(expected_roles)),
                    actual=wheel.name,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        if role in wheel_by_role:
            failures.append(
                _failure(
                    "build-host macOS wheel role is duplicated",
                    expected="one wheel for every declared macOS role",
                    actual=role,
                    repair="bash scripts/release.sh --candidate",
                )
            )
        wheel_by_role[role] = wheel
    records: list[dict[str, Any]] = []
    record_roles: set[str] = set()
    for path in host_result.native_records:
        role = _native_record_role(path)
        if role is None:
            failures.append(
                _failure(
                    "build-host native record role is invalid",
                    expected=", ".join(
                        sorted(
                            macos_native_record_name(role) for role in expected_roles
                        )
                    ),
                    actual=path.name,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        if role in record_roles:
            failures.append(
                _failure(
                    "build-host native record role is duplicated",
                    expected="one record for every declared macOS role",
                    actual=role,
                    repair="bash scripts/release.sh --candidate",
                )
            )
        record_roles.add(role)
        payload = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(payload, dict):
            failures.append(
                _failure(
                    "native record is not an object",
                    expected="JSON object",
                    actual=type(payload).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        wheel = wheel_by_role.get(role)
        if wheel is not None:
            failures.extend(
                validate_macos_native_record(
                    payload,
                    role=role,  # type: ignore[arg-type]
                    wheel_path=wheel,
                    source_commit=source_commit,
                    core_lock_sha256=core_lock_sha256,
                )
            )
        records.append(payload)
    if set(wheel_by_role) != expected_roles:
        failures.append(
            _failure(
                "build-host macOS wheel set is incomplete",
                expected=", ".join(sorted(expected_roles)),
                actual=", ".join(sorted(wheel_by_role)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if record_roles != expected_roles:
        failures.append(
            _failure(
                "build-host native record set is incomplete",
                expected=", ".join(sorted(expected_roles)),
                actual=", ".join(sorted(record_roles)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if failures:
        raise DriverError(failures)
    return records


def _native_records_by_role(
    native_records: Sequence[Mapping[str, Any]],
) -> dict[str, Mapping[str, Any]]:
    records: dict[str, Mapping[str, Any]] = {}
    failures: list[Failure] = []
    expected_roles = _expected_macos_roles()
    for record in native_records:
        role = record.get("role")
        if role not in expected_roles:
            failures.append(
                _failure(
                    "native record role is invalid",
                    expected=", ".join(sorted(expected_roles)),
                    actual=str(role),
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        if str(role) in records:
            failures.append(
                _failure(
                    "native record role is duplicated",
                    expected="one record for every declared macOS role",
                    actual=str(role),
                    repair="bash scripts/release.sh --candidate",
                )
            )
        records[str(role)] = record
    if set(records) != expected_roles:
        failures.append(
            _failure(
                "native record set is incomplete",
                expected=", ".join(sorted(expected_roles)),
                actual=", ".join(sorted(records)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if failures:
        raise DriverError(failures)
    return records


def _revalidate_macos_wheels(
    release_dir: Path,
    native_records: Sequence[Mapping[str, Any]],
    *,
    source_commit: str,
    core_lock_sha256: str,
) -> None:
    failures: list[Failure] = []
    records_by_role = _native_records_by_role(native_records)
    for role, record in sorted(records_by_role.items()):
        wheel = record.get("wheel")
        name = wheel.get("name") if isinstance(wheel, Mapping) else None
        if not isinstance(name, str):
            failures.append(
                _failure(
                    "native record wheel name is invalid",
                    expected=f"{role} native record wheel name",
                    actual=repr(wheel),
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        failures.extend(
            validate_macos_native_record(
                record,
                role=role,  # type: ignore[arg-type]
                wheel_path=release_dir / name,
                source_commit=source_commit,
                core_lock_sha256=core_lock_sha256,
            )
        )
    if failures:
        raise DriverError(failures)


def _validated_full_tool_evidence(
    coordinator_evidence: Mapping[str, Mapping[str, str]],
    host_result: BuildHostResult,
    native_records: Sequence[Mapping[str, Any]],
) -> dict[str, dict[str, str]]:
    failures: list[Failure] = []
    if "macos-arm64" in coordinator_evidence:
        failures.append(
            _failure(
                "macOS release tool evidence must be attested by the build host",
                expected="macos-arm64 absent from coordinator tool evidence",
                actual="macos-arm64 present",
                repair="bash scripts/release.sh --candidate",
            )
        )
    expected_coordinator_lanes = set(LANES) - {"macos-arm64"}
    if set(coordinator_evidence) - {"macos-arm64"} != expected_coordinator_lanes:
        failures.append(
            _failure(
                "coordinator release tool evidence lanes are invalid",
                expected=", ".join(sorted(expected_coordinator_lanes)),
                actual=", ".join(sorted(str(key) for key in coordinator_evidence))
                or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    combined: dict[str, dict[str, str]] = {}
    for lane, evidence in coordinator_evidence.items():
        if lane == "macos-arm64":
            continue
        if not isinstance(evidence, Mapping):
            failures.append(
                _failure(
                    "coordinator release tool evidence lane is invalid",
                    expected=f"{lane} tool evidence object",
                    actual=type(evidence).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        combined[str(lane)] = dict(evidence)
    if not isinstance(host_result.tool_evidence, Mapping):
        failures.append(
            _failure(
                "build-host macOS release tool evidence is unattested",
                expected="host-attested macOS tool evidence object",
                actual=type(host_result.tool_evidence).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        )
    else:
        final_macos, macos_failures = finalize_macos_tool_evidence(
            {str(key): str(value) for key, value in host_result.tool_evidence.items()},
            native_records,
        )
        failures.extend(macos_failures)
        if final_macos is not None:
            combined["macos-arm64"] = final_macos
    if set(combined) != set(LANES):
        failures.append(
            _failure(
                "release tool evidence lanes are incomplete",
                expected=", ".join(LANES),
                actual=", ".join(sorted(combined)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    for lane in sorted(set(combined) & set(LANE_TOOL_KEYS)):
        failures.extend(check_lane_tool_evidence(lane, combined[lane]))
    failures.extend(validate_public_evidence_tree("release_tool_evidence", combined))
    if failures:
        raise DriverError(failures)
    return combined


def _validate_coordinator_tool_evidence(
    coordinator_evidence: Mapping[str, Mapping[str, str]],
) -> dict[str, dict[str, str]]:
    failures: list[Failure] = []
    if "macos-arm64" in coordinator_evidence:
        failures.append(
            _failure(
                "macOS release tool evidence must be attested by the build host",
                expected="macos-arm64 absent from coordinator tool evidence",
                actual="macos-arm64 present",
                repair="bash scripts/release.sh --candidate",
            )
        )
    expected_lanes = set(LANES) - {"macos-arm64"}
    actual_lanes = set(coordinator_evidence) - {"macos-arm64"}
    if actual_lanes != expected_lanes:
        failures.append(
            _failure(
                "coordinator release tool evidence lanes are invalid",
                expected=", ".join(sorted(expected_lanes)),
                actual=", ".join(sorted(str(key) for key in actual_lanes)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    frozen: dict[str, dict[str, str]] = {}
    for lane in sorted(actual_lanes & expected_lanes):
        evidence = coordinator_evidence[lane]
        if not isinstance(evidence, Mapping):
            failures.append(
                _failure(
                    "coordinator release tool evidence lane is invalid",
                    expected=f"{lane} tool evidence object",
                    actual=type(evidence).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        frozen[lane] = {str(key): str(value) for key, value in evidence.items()}
        failures.extend(check_lane_tool_evidence(lane, frozen[lane]))
    failures.extend(validate_public_evidence_tree("coordinator_tool_evidence", frozen))
    if failures:
        raise DriverError(failures)
    return frozen


def _manifest_tools_from_full(
    lane: str,
    full_tool_evidence: Mapping[str, Mapping[str, str]],
) -> dict[str, str]:
    allowed = NATIVE_TOOL_KEYS[lane]  # type: ignore[index]
    tools = full_tool_evidence.get(lane, {})
    return {key: tools[key] for key in sorted(allowed)}


def _rustc_verbose_from_full_tool_evidence(
    lane: str, full_tool_evidence: Mapping[str, Mapping[str, str]]
) -> str:
    tools = full_tool_evidence[lane]
    host = {
        "source": "x86_64-unknown-linux-gnu",
        "linux-x86_64-musl": "x86_64-unknown-linux-gnu",
        "linux-aarch64-musl": "x86_64-unknown-linux-gnu",
        "macos-arm64": "aarch64-apple-darwin",
    }[lane]
    return "\n".join(
        [
            tools["rustc"],
            f"binary: {RUSTC_BINARY_PIN}",
            f"commit-hash: {RUSTC_COMMIT_HASH_PIN}",
            f"commit-date: {RUSTC_COMMIT_DATE_PIN}",
            f"host: {host}",
            f"release: {RUSTC_RELEASE_PIN}",
            f"LLVM version: {RUSTC_LLVM_PIN}",
        ]
    )


def _lane_evidence_from_full_tool_evidence(
    full_tool_evidence: Mapping[str, Mapping[str, str]],
    *,
    policy_run: PolicyRun,
) -> dict[str, LaneEvidence]:
    failures: list[Failure] = []
    if set(full_tool_evidence) != set(LANES):
        failures.append(
            _failure(
                "release tool evidence lanes are incomplete",
                expected=", ".join(LANES),
                actual=", ".join(sorted(str(key) for key in full_tool_evidence))
                or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    evidence_by_lane: dict[str, LaneEvidence] = {}
    for lane in sorted(set(full_tool_evidence) & set(LANES)):
        tools = full_tool_evidence[lane]
        try:
            native_tools = _manifest_tools_from_full(lane, full_tool_evidence)
            evidence_by_lane[lane] = LaneEvidence(
                rustc_verbose=_rustc_verbose_from_full_tool_evidence(
                    lane, full_tool_evidence
                ),
                cargo_version=tools["cargo"],
                native_tools=native_tools,
                cargo_deny_version=tools["cargo-deny"],
                advisory_checked_at=policy_run.policy_checked_at,
            )
        except KeyError as exc:
            failures.append(
                _failure(
                    "lane evidence cannot be derived from frozen tool observation",
                    expected=f"{lane} full release tool evidence contains manifest keys",
                    actual=str(exc),
                    repair="bash scripts/release.sh --candidate",
                )
            )
    if failures:
        raise DriverError(failures)
    return evidence_by_lane


def _copy_macos_wheels(host_result: BuildHostResult, dist_dir: Path) -> None:
    failures: list[Failure] = []
    expected_count = len(_expected_macos_roles())
    if len(host_result.macos_wheels) != expected_count:
        failures.append(
            _failure(
                "build-host macOS wheel set has wrong size",
                expected=f"exactly {expected_count} declared macOS wheels",
                actual=str(len(host_result.macos_wheels)),
                repair="bash scripts/release.sh --candidate",
            )
        )
    for wheel in host_result.macos_wheels:
        if not wheel.is_file() or wheel.is_symlink():
            failures.append(
                _failure(
                    "build-host macOS wheel is not a regular file",
                    expected="regular macOS wheel",
                    actual=wheel.name,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        shutil.copy2(wheel, dist_dir / wheel.name)
    if failures:
        raise DriverError(failures)


def _expected_members(
    ledger: Mapping[str, Any], target: str, *, release_dir: Path, schema_version: int
) -> tuple[Mapping[str, Mapping[str, Any]], list[Failure]]:
    native = ledger.get("native_members", {})
    if not isinstance(native, Mapping):
        return {}, [
            _failure(
                "retained ledger native members are invalid",
                expected="native_members object",
                actual=type(native).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    target_members = native.get(target)
    if not isinstance(target_members, Mapping):
        return {}, [
            _failure(
                "retained ledger target native members are invalid",
                expected=f"{target} native member object",
                actual=type(target_members).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    failures: list[Failure] = []
    members: dict[str, Mapping[str, Any]] = {}
    for member_name, member in target_members.items():
        if not isinstance(member, Mapping):
            failures.append(
                _failure(
                    "retained ledger native member is invalid",
                    expected="native member object",
                    actual=repr(member_name),
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        members[str(member_name)] = member
    expected, expected_failures = _expected_install_members(
        ledger,
        target,
        candidate_dir=release_dir,
        schema_version=schema_version,
    )
    return expected or members, [*failures, *expected_failures]


def _validate_proof_binding(
    proof: Mapping[str, Any],
    *,
    target: str,
    ledger: Mapping[str, Any],
    digest: str,
    ledger_sha256: str,
    release_dir: Path,
) -> list[Failure]:
    failures: list[Failure] = []
    proof_schema_version, proof_version_failures = _proof_schema_version(proof)
    if proof_schema_version is None:
        failures.extend(proof_version_failures)
        return failures
    expected_scalars = {
        "target": target,
        "source_commit": ledger.get("source_commit"),
        "candidate_digest": digest,
        "ledger_sha256": ledger_sha256,
        "core_lock_sha256": ledger.get("core_lock_sha256"),
    }
    for key, expected in expected_scalars.items():
        if proof.get(key) != expected:
            failures.append(
                _failure(
                    f"install proof {key} is not bound to retained candidate",
                    expected=str(expected),
                    actual=str(proof.get(key)),
                    repair="bash scripts/release.sh --recover",
                )
            )
    proof_entries: set[tuple[str, int, str]] = set()
    for entry in proof.get("candidate_files", []):
        if not isinstance(entry, Mapping):
            continue
        byte_count = entry.get("bytes", -1)
        if type(byte_count) is not int or byte_count < 0:
            failures.append(
                _failure(
                    "install proof candidate file byte count is invalid",
                    expected="non-negative integer",
                    actual=repr(byte_count),
                    repair=RETAINED_PROOF_REPAIR,
                )
            )
            continue
        proof_entries.add(
            (
                str(entry.get("basename")),
                byte_count,
                str(entry.get("sha256")),
            )
        )
    try:
        expected_install_paths = target_install_paths_from_ledger(
            ledger,
            target=target,
            candidate_dir=release_dir,
            schema_version=proof_schema_version,
        )
        expected_entries = {
            (
                str(entry["basename"]),
                int(entry["bytes"]),
                str(entry["sha256"]),
            )
            for entry in candidate_file_entries(expected_install_paths)
        }
    except InstallProofError as exc:
        failures.extend(exc.failures)
        expected_entries = set()
    if proof_entries != expected_entries:
        failures.append(
            _failure(
                "install proof candidate inventory does not match target install set",
                expected="retained ledger target install files",
                actual="proof candidate files differ",
                repair="bash scripts/release.sh --recover",
            )
        )
    expected_members, expected_member_failures = _expected_members(
        ledger, target, release_dir=release_dir, schema_version=proof_schema_version
    )
    failures.extend(expected_member_failures)
    installed_members = proof.get("installed_members", [])
    if not isinstance(installed_members, list):
        installed_members = []
    seen_members: set[str] = set()
    for member in installed_members:
        if not isinstance(member, Mapping):
            continue
        name = str(member.get("name"))
        seen_members.add(name)
        if "expected_sha256" in member:
            failures.append(
                _failure(
                    "install proof installed member carries forbidden expected hash",
                    expected="expected hashes retained only in ledger",
                    actual=name,
                    repair="bash scripts/release.sh --recover",
                )
            )
        expected = expected_members.get(name)
        if expected is None:
            failures.append(
                _failure(
                    "install proof installed member is not retained in payloads",
                    expected=f"{target} retained executable member",
                    actual=name,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if member.get("wheel_member_path") != expected.get("path"):
            failures.append(
                _failure(
                    "install proof wheel member path does not match ledger",
                    expected=str(expected.get("path")),
                    actual=str(member.get("wheel_member_path")),
                    repair="bash scripts/release.sh --recover",
                )
            )
        installed_path = member.get("installed_path")
        if not isinstance(installed_path, str) or not installed_path.startswith(
            f"{ENVROOT}/"
        ):
            failures.append(
                _failure(
                    "install proof installed path is invalid",
                    expected=f"normalized installed executable path beneath {ENVROOT}",
                    actual=repr(installed_path),
                    repair="bash scripts/release.sh --recover",
                )
            )
        if installed_path == member.get("wheel_member_path"):
            failures.append(
                _failure(
                    "install proof member paths are conflated",
                    expected="distinct wheel_member_path and installed_path",
                    actual=repr(installed_path),
                    repair="bash scripts/release.sh --recover",
                )
            )
        if member.get("sha256") != expected.get("sha256"):
            failures.append(
                _failure(
                    "install proof installed member hash does not match ledger",
                    expected=str(expected.get("sha256")),
                    actual=str(member.get("sha256")),
                    repair="bash scripts/release.sh --recover",
                )
            )
    expected_names = set(expected_members)
    if seen_members != expected_names:
        failures.append(
            _failure(
                "install proof installed member set does not match ledger",
                expected=", ".join(sorted(expected_names)) or "<empty>",
                actual=", ".join(sorted(seen_members)) or "<empty>",
                repair="bash scripts/release.sh --recover",
            )
        )
    return failures


def _validate_models_binding(
    root: Path,
    release_dir: Path,
    ledger: Mapping[str, Any],
    *,
    check_local_version: bool = True,
) -> list[Failure]:
    models = ledger.get("models")
    if not isinstance(models, Mapping):
        return [
            _failure(
                "retained ledger models binding is invalid",
                expected="models decision object",
                actual=type(models).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    decision = models.get("decision")
    package_version = models.get("package_version")
    if decision not in {"include", "exclude"}:
        return [
            _failure(
                "retained ledger models decision is invalid",
                expected="include or exclude",
                actual=str(decision),
                repair="bash scripts/release.sh --recover",
            )
        ]
    failures: list[Failure] = []
    if check_local_version:
        actual_version = _local_models_package_version(root)
        if package_version != actual_version:
            failures.append(
                _failure(
                    "retained ledger models package version changed",
                    expected=str(package_version),
                    actual=actual_version,
                    repair="bash scripts/release.sh --recover",
                )
            )
    expected = set(expected_package_names(include_models=decision == "include"))
    actual = {path.name for path in release_dir.iterdir() if path.is_file()}
    manifests = {
        name for name in actual if name.endswith(".rust-release-manifest.json")
    }
    package_names = actual - manifests
    if package_names != expected:
        failures.append(
            _failure(
                "retained candidate package inventory does not match models decision",
                expected=", ".join(sorted(expected)),
                actual=", ".join(sorted(package_names)) or "<empty>",
                repair="bash scripts/release.sh --recover",
            )
        )
    return failures


def _expected_payload_file_names(*, include_models: bool) -> frozenset[str]:
    packages = set(expected_package_names(include_models=include_models))
    manifests = {
        f"{name}.rust-release-manifest.json" for name in rust_artifact_targets()
    }
    return frozenset(packages | manifests)


def _safe_retained_basename(value: Any) -> bool:
    return (
        isinstance(value, str)
        and bool(value)
        and value not in {".", ".."}
        and Path(value).name == value
        and "/" not in value
        and "\\" not in value
    )


def _canonical_nvattest_authority_bytes(payload: Mapping[str, Any]) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _driver_failures_from_support_error(exc: SupportLockError) -> list[Failure]:
    return [
        _failure(
            failure.error,
            expected=failure.expected,
            actual=failure.actual,
            repair=failure.repair,
        )
        for failure in exc.failures
    ]


def _driver_failures_from_nvattest_error(exc: NvattestProofError) -> list[Failure]:
    return [
        _failure(
            failure.error,
            expected=failure.expected,
            actual=failure.actual,
            repair=failure.repair,
        )
        for failure in exc.failures
    ]


def _validate_nvattest_challenge(value: str) -> list[Failure]:
    if CHALLENGE_RE.fullmatch(value):
        return []
    return [
        _failure(
            "nvattest challenge is invalid",
            expected="64 lowercase hexadecimal characters",
            actual=repr(value),
            repair="bash scripts/release.sh --candidate",
        )
    ]


def _support_destination_failures(
    support_dir: Path,
    paths: Sequence[Path],
    expected_entries: Sequence[SupportLockEntry],
) -> list[Failure]:
    failures: list[Failure] = []
    expected_names = {entry.filename for entry in expected_entries}
    try:
        support_root = support_dir.resolve()
    except OSError:
        support_root = support_dir
    for raw_path in paths:
        path = Path(raw_path)
        try:
            path.resolve().relative_to(support_root)
        except (OSError, ValueError):
            failures.append(
                _failure(
                    "nvattest support materialization escaped destination",
                    expected="support wheel path under evidence support directory",
                    actual=path.name,
                    repair="bash scripts/release.sh --candidate",
                )
            )
    try:
        support_entries = {path.name: path for path in support_dir.iterdir()}
    except FileNotFoundError:
        return [
            _failure(
                "nvattest support inventory is not exact",
                expected=", ".join(sorted(expected_names)),
                actual="<missing>",
                repair="bash scripts/release.sh --candidate",
            )
        ]
    if set(support_entries) != expected_names:
        failures.append(
            _failure(
                "nvattest support inventory is not exact",
                expected=", ".join(sorted(expected_names)),
                actual=", ".join(sorted(support_entries)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    for name, path in sorted(support_entries.items()):
        try:
            entry = path.lstat()
        except OSError as exc:
            failures.append(
                _failure(
                    "nvattest support wheel could not be inspected",
                    expected=f"regular support wheel {name}",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        if stat.S_ISLNK(entry.st_mode) or not stat.S_ISREG(entry.st_mode):
            failures.append(
                _failure(
                    "nvattest support wheel is not a regular file",
                    expected=f"regular support wheel {name}",
                    actual="non-regular",
                    repair="bash scripts/release.sh --candidate",
                )
            )
    return failures


def _materialize_verified_support_wheels(
    *,
    root: Path,
    support_dir: Path,
    services: CandidateServices,
) -> tuple[tuple[dict[str, object], ...], tuple[Path, ...]]:
    try:
        expected_entries = read_support_lock_entries(root / "uv.lock")
        expected_support = support_declarations_from_lock(expected_entries)
    except SupportLockError as exc:
        raise DriverError(_driver_failures_from_support_error(exc)) from None
    try:
        materialized = tuple(
            Path(path) for path in services.materialize_support_wheels(support_dir)
        )
    except Exception as exc:
        raise DriverError(
            [
                _failure(
                    "nvattest support materialization failed",
                    expected="support wheels materialized into evidence support directory",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from None
    materialization_failures = _support_destination_failures(
        support_dir, materialized, expected_entries
    )
    if materialization_failures:
        raise DriverError(materialization_failures)
    support_paths = tuple(
        path.resolve()
        for path in sorted(support_dir.iterdir(), key=lambda path: path.name)
    )
    try:
        observed = verify_support_wheels_against_lock(support_paths, expected_entries)
    except SupportLockError as exc:
        raise DriverError(_driver_failures_from_support_error(exc)) from None
    if observed != expected_support:
        raise DriverError(
            [
                _failure(
                    "nvattest support declaration is not bound to release lock",
                    expected=repr(expected_support),
                    actual=repr(observed),
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return expected_support, support_paths


def _root_wheel_name_for_target(
    target: str,
    names: Sequence[str],
) -> tuple[str | None, list[Failure]]:
    selected = _select_names_for_target(
        target, names, schema_version=CURRENT_PROOF_SCHEMA_VERSION
    )
    root_wheels = [name for name in selected if name.startswith("solstone-")]
    if len(root_wheels) == 1:
        return root_wheels[0], []
    return None, [
        _failure(
            "candidate nvattest root wheel selection is invalid",
            expected=f"exactly one retained root wheel for {target}",
            actual=", ".join(root_wheels) or "<empty>",
            repair="bash scripts/release.sh --recover",
        )
    ]


def _extract_nvattest_authority_bytes(
    release_dir: Path,
    names: Sequence[str],
) -> tuple[bytes | None, list[Failure]]:
    failures: list[Failure] = []
    by_target: dict[str, bytes] = {}
    for target in PROOF_TARGETS:
        root_name, target_failures = _root_wheel_name_for_target(target, names)
        failures.extend(target_failures)
        if root_name is None:
            continue
        wheel_path = release_dir / root_name
        try:
            with zipfile.ZipFile(wheel_path) as wheel:
                by_target[target] = wheel.read(NVATTEST_AUTHORITY_MEMBER)
        except KeyError:
            failures.append(
                _failure(
                    "candidate nvattest authority member is missing",
                    expected=f"{root_name}:{NVATTEST_AUTHORITY_MEMBER}",
                    actual="<missing>",
                    repair="bash scripts/release.sh --recover",
                )
            )
        except (OSError, zipfile.BadZipFile) as exc:
            failures.append(
                _failure(
                    "candidate nvattest authority member could not be read",
                    expected=f"readable {NVATTEST_AUTHORITY_MEMBER}",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
    distinct = {bytes_value for bytes_value in by_target.values()}
    if len(distinct) > 1:
        failures.append(
            _failure(
                "candidate nvattest authority members differ by target",
                expected="byte-identical authority member in every root wheel",
                actual=", ".join(sorted(by_target)),
                repair="bash scripts/release.sh --recover",
            )
        )
    return (next(iter(distinct)) if len(distinct) == 1 else None), failures


def _candidate_names_from_dir(release_dir: Path) -> tuple[str, ...]:
    return tuple(sorted(path.name for path in release_dir.iterdir() if path.is_file()))


def _nvattest_ledger_payload(
    *,
    challenge: str,
    authority_bytes: bytes,
    support_distributions: Sequence[Mapping[str, object]],
) -> dict[str, object]:
    failures = _validate_nvattest_challenge(challenge)
    try:
        authority = json.loads(authority_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        failures.append(
            _failure(
                "candidate nvattest authority member is not valid JSON",
                expected="canonical authority JSON object",
                actual=str(exc),
                repair="bash scripts/release.sh --candidate",
            )
        )
        authority = {}
    if not isinstance(authority, Mapping):
        failures.append(
            _failure(
                "candidate nvattest authority member is not an object",
                expected="canonical authority JSON object",
                actual=type(authority).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        )
        authority = {}
    elif _canonical_nvattest_authority_bytes(authority) != authority_bytes:
        failures.append(
            _failure(
                "candidate nvattest authority member is not canonical",
                expected="canonical sorted authority JSON bytes",
                actual="authority bytes differ",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if failures:
        raise DriverError(failures)
    return {
        "authority": dict(authority),
        "authority_sha256": hashlib.sha256(authority_bytes).hexdigest(),
        "challenge": challenge,
        "support_distributions": [dict(entry) for entry in support_distributions],
    }


def _schema_absent_nvattest_failure(version: int) -> Failure:
    return _failure(
        "retained ledger schema does not declare nvattest",
        expected="schema with nvattest binding",
        actual=f"schema_version {version}",
        repair="bash scripts/release.sh --recover",
    )


def _resolve_driver_retained_ledger_schema(
    ledger: Mapping[str, Any],
) -> tuple[int, RetainedLedgerSchema]:
    try:
        return resolve_retained_ledger_schema(ledger)
    except LedgerError as exc:
        raise DriverError(exc.failures) from None


def _require_nvattest_schema(
    ledger: Mapping[str, Any],
) -> tuple[int, RetainedLedgerSchema]:
    version, schema = _resolve_driver_retained_ledger_schema(ledger)
    if not retained_ledger_schema_declares_nvattest(schema):
        raise DriverError([_schema_absent_nvattest_failure(version)])
    return version, schema


def _retained_support_binding_failures(
    *,
    evidence_dir: Path,
    ledger: Mapping[str, Any],
) -> list[Failure]:
    _require_nvattest_schema(ledger)
    failures: list[Failure] = []
    nvattest = ledger.get("nvattest")
    if not isinstance(nvattest, Mapping):
        return [
            _failure(
                "retained ledger nvattest binding is invalid",
                expected="nvattest object",
                actual=type(nvattest).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    support_distributions = nvattest.get("support_distributions")
    declaration_failures = validate_support_declarations(
        support_distributions,
        repair="bash scripts/release.sh --recover",
    )
    if declaration_failures:
        return declaration_failures
    assert isinstance(support_distributions, Sequence)
    expected = [
        dict(entry) for entry in support_distributions if isinstance(entry, Mapping)
    ]
    expected_names = {str(entry["filename"]) for entry in expected}
    support_dir = evidence_dir / "support"
    actual_paths = {path.name: path for path in support_dir.iterdir()}
    if set(actual_paths) != expected_names:
        failures.append(
            _failure(
                "retained nvattest support inventory disagrees with ledger",
                expected=", ".join(sorted(expected_names)),
                actual=", ".join(sorted(actual_paths)) or "<empty>",
                repair="bash scripts/release.sh --recover",
            )
        )
    support_paths = tuple(
        actual_paths[name].resolve()
        for name in sorted(set(actual_paths) & expected_names)
    )
    if support_paths:
        try:
            observed = support_distribution_entries(support_paths)
        except NvattestProofError as exc:
            failures.extend(_driver_failures_from_nvattest_error(exc))
            observed = []
        if observed != expected:
            failures.append(
                _failure(
                    "retained nvattest support bytes disagree with ledger",
                    expected=repr(expected),
                    actual=repr(observed),
                    repair="bash scripts/release.sh --recover",
                )
            )
    return failures


def _retained_authority_binding(
    *,
    release_dir: Path,
    ledger: Mapping[str, Any],
) -> tuple[bytes | None, list[Failure]]:
    _require_nvattest_schema(ledger)
    names, name_failures = _ledger_candidate_file_names(ledger)
    if name_failures:
        return None, name_failures
    authority_bytes, failures = _extract_nvattest_authority_bytes(
        release_dir, sorted(names)
    )
    nvattest = ledger.get("nvattest")
    if not isinstance(nvattest, Mapping):
        failures.append(
            _failure(
                "retained ledger nvattest binding is invalid",
                expected="nvattest object",
                actual=type(nvattest).__name__,
                repair="bash scripts/release.sh --recover",
            )
        )
        return authority_bytes, failures
    authority = nvattest.get("authority")
    authority_sha256 = nvattest.get("authority_sha256")
    if not isinstance(authority, Mapping):
        failures.append(
            _failure(
                "retained nvattest authority is invalid",
                expected="ledger nvattest authority object",
                actual=type(authority).__name__,
                repair="bash scripts/release.sh --recover",
            )
        )
        return authority_bytes, failures
    if authority_bytes is not None:
        canonical = _canonical_nvattest_authority_bytes(authority)
        if canonical != authority_bytes:
            failures.append(
                _failure(
                    "retained nvattest authority disagrees with candidate wheels",
                    expected="ledger authority canonical bytes match retained root wheels",
                    actual="authority bytes differ",
                    repair="bash scripts/release.sh --recover",
                )
            )
        if hashlib.sha256(authority_bytes).hexdigest() != authority_sha256:
            failures.append(
                _failure(
                    "retained nvattest authority digest disagrees with candidate wheels",
                    expected=str(authority_sha256),
                    actual=hashlib.sha256(authority_bytes).hexdigest(),
                    repair="bash scripts/release.sh --recover",
                )
            )
    return authority_bytes, failures


def _retained_install_proof_schema_version(
    *, evidence_dir: Path, target: str
) -> tuple[int | None, list[Failure]]:
    path = evidence_dir / "proofs" / f"{target}.json"
    if not path.is_file() or path.is_symlink():
        return None, [
            _failure(
                "retained install proof is missing for schema dispatch",
                expected=f"{target} install proof",
                actual="missing",
                repair="bash scripts/release.sh --recover",
            )
        ]
    try:
        proof = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return None, [
            _failure(
                "retained install proof schema could not be read",
                expected=f"{target} install proof JSON",
                actual=str(exc),
                repair="bash scripts/release.sh --recover",
            )
        ]
    if not isinstance(proof, Mapping):
        return None, [
            _failure(
                "retained install proof schema payload is invalid",
                expected=f"{target} install proof object",
                actual=type(proof).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    return _proof_schema_version(proof)


def _validate_retained_nvattest_binding(
    *,
    evidence_dir: Path,
    ledger: Mapping[str, Any],
    digest: str,
    ledger_sha256: str,
    release_dir: Path,
    version: str,
) -> dict[str, str]:
    _require_nvattest_schema(ledger)
    failures: list[Failure] = []
    nvattest = ledger.get("nvattest")
    if not isinstance(nvattest, Mapping):
        raise DriverError(
            [
                _failure(
                    "retained ledger nvattest binding is invalid",
                    expected="nvattest object",
                    actual=type(nvattest).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            ]
        )

    authority_bytes, authority_failures = _retained_authority_binding(
        release_dir=release_dir,
        ledger=ledger,
    )
    failures.extend(authority_failures)
    challenge = nvattest.get("challenge")
    if not isinstance(challenge, str) or not CHALLENGE_RE.fullmatch(challenge):
        failures.append(
            _failure(
                "retained nvattest challenge is invalid",
                expected="64 lowercase hexadecimal characters",
                actual=repr(challenge),
                repair="bash scripts/release.sh --recover",
            )
        )

    support_distributions = nvattest.get("support_distributions")
    support_declaration_failures = validate_support_declarations(
        support_distributions,
        repair="bash scripts/release.sh --recover",
    )
    failures.extend(support_declaration_failures)
    if not support_declaration_failures:
        failures.extend(
            _retained_support_binding_failures(evidence_dir=evidence_dir, ledger=ledger)
        )
    if support_declaration_failures or authority_bytes is None:
        raise DriverError(failures)

    assert isinstance(support_distributions, Sequence)
    expected_support_distributions: Sequence[Mapping[str, Any]] = ()
    support_closure_inputs_ready = True
    try:
        expected_support_distributions = support_distribution_entries_with_metadata(
            tuple(
                (evidence_dir / "support" / str(entry["filename"])).resolve()
                for entry in support_distributions
                if isinstance(entry, Mapping)
            )
        )
    except NvattestProofError as exc:
        failures.extend(_driver_failures_from_nvattest_error(exc))
        support_closure_inputs_ready = False
    hashes: dict[str, str] = {}
    for target in PROOF_TARGETS:
        path = evidence_dir / "nvattest" / f"{target}.json"
        if not path.is_file() or path.is_symlink():
            failures.append(
                _failure(
                    "release nvattest receipt is missing",
                    expected=f"{target} nvattest receipt",
                    actual="missing",
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        data = path.read_bytes()
        expected_candidate_wheels: Sequence[Mapping[str, Any]] = ()
        candidate_closure_inputs_ready = True
        try:
            proof_schema_version, proof_schema_failures = (
                _retained_install_proof_schema_version(
                    evidence_dir=evidence_dir, target=target
                )
            )
            if proof_schema_version is None:
                failures.extend(proof_schema_failures)
                candidate_closure_inputs_ready = False
                continue
            expected_candidate_wheels = candidate_wheel_entries(
                target_install_paths_from_ledger(
                    ledger,
                    target=target,
                    candidate_dir=release_dir,
                    schema_version=proof_schema_version,
                )
            )
        except NvattestProofError as exc:
            failures.extend(_driver_failures_from_nvattest_error(exc))
            candidate_closure_inputs_ready = False
        except InstallProofError as exc:
            failures.extend(exc.failures)
            candidate_closure_inputs_ready = False
        if support_closure_inputs_ready and candidate_closure_inputs_ready:
            failures.extend(
                validate_nvattest_proof_bytes(
                    data,
                    expected_challenge=challenge if isinstance(challenge, str) else "",
                    target=target,
                    version=version,
                    source_commit=str(ledger.get("source_commit")),
                    core_lock_sha256=str(ledger.get("core_lock_sha256")),
                    candidate_digest=digest,
                    ledger_sha256=ledger_sha256,
                    canonical_authority_bytes=authority_bytes,
                    expected_candidate_wheels=expected_candidate_wheels,
                    expected_support_distributions=expected_support_distributions,
                )
            )
        try:
            receipt = json.loads(data.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            receipt = None
        installed_authority = (
            receipt.get("installed_authority") if isinstance(receipt, Mapping) else None
        )
        if isinstance(installed_authority, Mapping) and installed_authority.get(
            "size_bytes"
        ) != len(authority_bytes):
            failures.append(
                _failure(
                    "nvattest receipt installed authority size disagrees with candidate wheels",
                    expected=str(len(authority_bytes)),
                    actual=str(installed_authority.get("size_bytes")),
                    repair="bash scripts/release.sh --recover",
                )
            )
        hashes[target] = file_sha256_size(path)[0]
    extras = sorted(
        path.name
        for path in (evidence_dir / "nvattest").glob("*.json")
        if path.stem not in PROOF_TARGETS
    )
    if extras:
        failures.append(
            _failure(
                "release nvattest set has extra targets",
                expected=", ".join(PROOF_TARGETS),
                actual=", ".join(extras),
                repair="bash scripts/release.sh --recover",
            )
        )
    if failures:
        raise DriverError(failures)
    return hashes


def _ledger_candidate_file_names(
    ledger: Mapping[str, Any],
) -> tuple[frozenset[str], list[Failure]]:
    candidate = ledger.get("candidate")
    if not isinstance(candidate, Mapping):
        return frozenset(), [
            _failure(
                "retained ledger candidate inventory is invalid",
                expected="candidate object",
                actual=type(candidate).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    files = candidate.get("files")
    if not isinstance(files, list):
        return frozenset(), [
            _failure(
                "retained ledger candidate file list is invalid",
                expected="candidate.files list",
                actual=type(files).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    names: list[str] = []
    failures: list[Failure] = []
    for index, item in enumerate(files):
        name = item.get("name") if isinstance(item, Mapping) else None
        if not _safe_retained_basename(name):
            failures.append(
                _failure(
                    "retained ledger candidate filename is invalid",
                    expected=f"candidate.files[{index}].name safe basename",
                    actual=repr(name),
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        names.append(str(name))
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        failures.append(
            _failure(
                "retained ledger candidate file list has duplicate names",
                expected="unique retained candidate file basenames",
                actual=", ".join(duplicates),
                repair="bash scripts/release.sh --recover",
            )
        )
    return frozenset(names), failures


def _validate_flat_payload_inventory(
    release_dir: Path,
    *,
    include_models: bool | None = None,
    expected_names: frozenset[str] | None = None,
) -> list[Failure]:
    failures: list[Failure] = []
    try:
        root_entry = release_dir.lstat()
    except FileNotFoundError:
        return [
            _failure(
                "release payload directory is missing",
                expected="final release payload directory",
                actual="missing",
                repair="bash scripts/release.sh --recover",
            )
        ]
    if stat.S_ISLNK(root_entry.st_mode) or not stat.S_ISDIR(root_entry.st_mode):
        return [
            _failure(
                "release payload is not an owned directory",
                expected="non-symlink release payload directory",
                actual=release_dir.name,
                repair="bash scripts/release.sh --recover",
            )
        ]
    names: set[str] = set()
    for path in release_dir.iterdir():
        entry = path.lstat()
        if not stat.S_ISREG(entry.st_mode):
            failures.append(
                _failure(
                    "release payload inventory contains unsafe entry",
                    expected="flat regular payload files only",
                    actual=path.name,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        names.add(path.name)
    if expected_names is None:
        if include_models is None:
            raise AssertionError("include_models is required without expected_names")
        expected = _expected_payload_file_names(include_models=include_models)
    else:
        expected = expected_names
    if names != expected:
        failures.append(
            _failure(
                "release payload inventory is not exact",
                expected=", ".join(sorted(expected)),
                actual=", ".join(sorted(names)) or "<empty>",
                repair="bash scripts/release.sh --recover",
            )
        )
    return failures


def _retained_rust_targets_by_artifact(
    ledger: Mapping[str, Any],
) -> tuple[dict[str, tuple[str, dict[str, Any]]], list[Failure]]:
    raw_targets = ledger.get("rust_targets")
    if not isinstance(raw_targets, list):
        return {}, [
            _failure(
                "retained ledger Rust targets are invalid",
                expected="rust_targets list",
                actual=type(raw_targets).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    targets: dict[str, tuple[str, dict[str, Any]]] = {}
    failures: list[Failure] = []
    for index, item in enumerate(raw_targets):
        if not isinstance(item, Mapping):
            failures.append(
                _failure(
                    "retained ledger Rust target entry is invalid",
                    expected=f"rust_targets[{index}] object",
                    actual=type(item).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        artifact = item.get("artifact")
        lane = item.get("lane")
        if not _safe_retained_basename(artifact) or lane not in LANES:
            failures.append(
                _failure(
                    "retained ledger Rust target entry is invalid",
                    expected=f"rust_targets[{index}] artifact basename and lane",
                    actual=repr(item),
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        target = {
            str(key): value
            for key, value in item.items()
            if key not in {"artifact", "lane"}
        }
        if str(artifact) in targets:
            failures.append(
                _failure(
                    "retained ledger Rust targets contain duplicate artifacts",
                    expected="one Rust target per retained artifact",
                    actual=str(artifact),
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        targets[str(artifact)] = (str(lane), target)
    return targets, failures


def _manifest_artifact_payload(
    manifest_name: str, payload: Mapping[str, Any]
) -> tuple[Mapping[str, Any] | None, str | None, list[Failure]]:
    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list) or len(artifacts) != 1:
        return (
            None,
            None,
            [
                _failure(
                    "retained manifest must contain exactly one artifact",
                    expected=f"{manifest_name} one artifact entry",
                    actual=repr(artifacts),
                    repair="bash scripts/release.sh --recover",
                )
            ],
        )
    artifact = artifacts[0]
    if not isinstance(artifact, Mapping):
        return (
            None,
            None,
            [
                _failure(
                    "retained manifest artifact entry is invalid",
                    expected=f"{manifest_name} artifact object",
                    actual=type(artifact).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            ],
        )
    artifact_name = artifact.get("path")
    if not _safe_retained_basename(artifact_name):
        return (
            artifact,
            None,
            [
                _failure(
                    "retained manifest artifact path is invalid",
                    expected=f"{manifest_name} artifact basename",
                    actual=repr(artifact_name),
                    repair="bash scripts/release.sh --recover",
                )
            ],
        )
    return artifact, str(artifact_name), []


def _expected_manifest_native_tools(
    ledger: Mapping[str, Any], lane: str
) -> Mapping[str, str] | None:
    tool_evidence = ledger.get("tool_evidence")
    if not isinstance(tool_evidence, Mapping):
        return None
    lane_tools = tool_evidence.get(lane)
    if not isinstance(lane_tools, Mapping) or lane not in NATIVE_TOOL_KEYS:
        return None
    return {
        key: str(lane_tools[key])
        for key in NATIVE_TOOL_KEYS[lane]
        if isinstance(lane_tools.get(key), str)
    }


def _validate_retained_manifest_files(
    release_dir: Path,
    ledger: Mapping[str, Any],
) -> list[Failure]:
    expected_names, failures = _ledger_candidate_file_names(ledger)
    manifest_names = sorted(
        name for name in expected_names if name.endswith(".rust-release-manifest.json")
    )
    if len(manifest_names) != 4:
        failures.append(
            _failure(
                "retained payload manifest count is invalid",
                expected="four retained Rust companion manifests",
                actual=str(len(manifest_names)),
                repair="bash scripts/release.sh --recover",
            )
        )
    rust_targets, target_failures = _retained_rust_targets_by_artifact(ledger)
    failures.extend(target_failures)
    dependency_policy = ledger.get("dependency_policy")
    expected_scalars = {
        "product": "solstone-core",
        "version": ledger.get("version"),
        "source_commit": ledger.get("source_commit"),
        "source_dirty": False,
        "cargo_lock_sha256": ledger.get("core_lock_sha256"),
        "dependency_policy": dependency_policy,
        "active_exceptions": [],
    }
    for manifest_name in manifest_names:
        manifest_path = release_dir / manifest_name
        try:
            entry = manifest_path.lstat()
        except OSError as exc:
            failures.append(
                _failure(
                    "retained manifest could not be inspected",
                    expected=f"{manifest_name} regular file",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if not stat.S_ISREG(entry.st_mode):
            failures.append(
                _failure(
                    "retained manifest is not a regular file",
                    expected=f"{manifest_name} regular file",
                    actual="non-regular",
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        try:
            payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
            failures.append(
                _failure(
                    "retained manifest is not readable JSON",
                    expected=f"{manifest_name} JSON object",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if not isinstance(payload, Mapping):
            failures.append(
                _failure(
                    "retained manifest is not a JSON object",
                    expected=f"{manifest_name} object",
                    actual=type(payload).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        for key, expected in expected_scalars.items():
            if payload.get(key) != expected:
                failures.append(
                    _failure(
                        f"retained manifest {key} is not bound to ledger",
                        expected=repr(expected),
                        actual=repr(payload.get(key)),
                        repair="bash scripts/release.sh --recover",
                    )
                )
        artifact, artifact_name, artifact_failures = _manifest_artifact_payload(
            manifest_name, payload
        )
        failures.extend(artifact_failures)
        if artifact is None or artifact_name is None:
            continue
        if manifest_name != f"{artifact_name}.rust-release-manifest.json":
            failures.append(
                _failure(
                    "retained manifest filename is not the artifact companion name",
                    expected=f"{artifact_name}.rust-release-manifest.json",
                    actual=manifest_name,
                    repair="bash scripts/release.sh --recover",
                )
            )
        if artifact_name not in expected_names:
            failures.append(
                _failure(
                    "retained manifest artifact is not in retained candidate inventory",
                    expected="artifact named by ledger candidate files",
                    actual=artifact_name,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        try:
            artifact_sha256, artifact_bytes = file_sha256_size(
                release_dir / artifact_name
            )
        except OSError as exc:
            failures.append(
                _failure(
                    "retained manifest artifact could not be read",
                    expected=artifact_name,
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if (
            artifact.get("sha256") != artifact_sha256
            or artifact.get("bytes") != artifact_bytes
        ):
            failures.append(
                _failure(
                    "retained manifest artifact digest does not match final bytes",
                    expected=f"{artifact_sha256}/{artifact_bytes}",
                    actual=repr(artifact),
                    repair="bash scripts/release.sh --recover",
                )
            )
        retained_target = rust_targets.get(artifact_name)
        if retained_target is None:
            failures.append(
                _failure(
                    "retained manifest artifact has no retained Rust target",
                    expected=artifact_name,
                    actual="missing",
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        lane, target = retained_target
        if payload.get("target") != target:
            failures.append(
                _failure(
                    "retained manifest target does not match ledger Rust target",
                    expected=repr(target),
                    actual=repr(payload.get("target")),
                    repair="bash scripts/release.sh --recover",
                )
            )
        expected_native_tools = _expected_manifest_native_tools(ledger, lane)
        if payload.get("native_tools") != expected_native_tools:
            failures.append(
                _failure(
                    "retained manifest native tools do not match ledger tool evidence",
                    expected=repr(expected_native_tools),
                    actual=repr(payload.get("native_tools")),
                    repair="bash scripts/release.sh --recover",
                )
            )
    return failures


def _validate_publication_prerequisite_record(
    path: Path,
    *,
    version: str,
) -> list[Failure]:
    entry = path.lstat()
    repair = (
        "publish and verify solstone-core-unsupported-platform before publishing "
        "solstone"
    )
    if stat.S_ISLNK(entry.st_mode):
        return [
            _failure(
                "core unsupported-platform tombstone prerequisite is a symlink",
                expected="regular non-symlink publication prerequisite record",
                actual="symlink",
                repair=repair,
            )
        ]
    if not stat.S_ISREG(entry.st_mode):
        return [
            _failure(
                "core unsupported-platform tombstone prerequisite is not a regular file",
                expected="regular publication prerequisite record",
                actual="non-regular",
                repair=repair,
            )
        ]
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        return [
            _failure(
                "core unsupported-platform tombstone prerequisite could not be read",
                expected="readable UTF-8 publication verification JSON",
                actual=type(exc).__name__,
                repair=repair,
            )
        ]
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        return [
            _failure(
                "core unsupported-platform tombstone prerequisite is not valid JSON",
                expected="publication verification JSON object",
                actual=str(exc),
                repair=repair,
            )
        ]
    return validate_core_unsupported_tombstone_record(payload, version=version)


def _required_evidence_entries(schema: RetainedLedgerSchema) -> set[str]:
    entries = {"ledger.json", "proofs"}
    if retained_ledger_schema_declares_nvattest(schema):
        entries.update(("nvattest", "support"))
    return entries


def _evidence_inventory_text(entries: set[str]) -> str:
    return ", ".join(sorted(entries))


def _validate_evidence_inventory(
    evidence_dir: Path,
    *,
    schema_version: int,
    schema: RetainedLedgerSchema,
    publication_prerequisite_version: str | None = None,
) -> list[Failure]:
    failures: list[Failure] = []
    required_entries = _required_evidence_entries(schema)
    expected_inventory = _evidence_inventory_text(required_entries)
    try:
        evidence_entry = evidence_dir.lstat()
    except FileNotFoundError:
        return [
            _failure(
                "release evidence directory is missing",
                expected=expected_inventory,
                actual="missing",
                repair="bash scripts/release.sh --recover",
            )
        ]
    if stat.S_ISLNK(evidence_entry.st_mode) or not stat.S_ISDIR(evidence_entry.st_mode):
        return [
            _failure(
                "release evidence is not an owned directory",
                expected="non-symlink evidence directory",
                actual=evidence_dir.name,
                repair="bash scripts/release.sh --recover",
            )
        ]
    entries = {path.name: path for path in evidence_dir.iterdir()}
    accepted_entries = set(required_entries)
    if publication_prerequisite_version is not None:
        accepted_entries.add(CORE_UNSUPPORTED_TOMBSTONE_RECORD)
        expected_inventory = (
            f"{expected_inventory}, optional {CORE_UNSUPPORTED_TOMBSTONE_RECORD}"
        )
    actual_entries = set(entries)
    if not required_entries <= actual_entries <= accepted_entries:
        failures.append(
            _failure(
                "release evidence inventory is not exact",
                expected=f"schema_version {schema_version}: {expected_inventory}",
                actual=", ".join(sorted(entries)) or "<empty>",
                repair="bash scripts/release.sh --recover",
            )
        )
    ledger_path = entries.get("ledger.json")
    if ledger_path is not None:
        ledger_entry = ledger_path.lstat()
        if not stat.S_ISREG(ledger_entry.st_mode):
            failures.append(
                _failure(
                    "release ledger is not a regular file",
                    expected="regular ledger.json",
                    actual="non-regular",
                    repair="bash scripts/release.sh --recover",
                )
            )
    proofs_dir = entries.get("proofs")
    if proofs_dir is not None:
        proofs_entry = proofs_dir.lstat()
        if stat.S_ISLNK(proofs_entry.st_mode) or not stat.S_ISDIR(proofs_entry.st_mode):
            failures.append(
                _failure(
                    "release proofs entry is not an owned directory",
                    expected="non-symlink proofs directory",
                    actual="non-directory",
                    repair="bash scripts/release.sh --recover",
                )
            )
        else:
            proof_entries = {path.name: path for path in proofs_dir.iterdir()}
            expected_proofs = {f"{target}.json" for target in PROOF_TARGETS}
            if set(proof_entries) != expected_proofs:
                failures.append(
                    _failure(
                        "release proof inventory is not exact",
                        expected=", ".join(sorted(expected_proofs)),
                        actual=", ".join(sorted(proof_entries)) or "<empty>",
                        repair="bash scripts/release.sh --recover",
                    )
                )
            for proof_name, proof_path in sorted(proof_entries.items()):
                proof_entry = proof_path.lstat()
                if not stat.S_ISREG(proof_entry.st_mode):
                    failures.append(
                        _failure(
                            "release proof is not a regular file",
                            expected=f"regular proof file {proof_name}",
                            actual="non-regular",
                            repair="bash scripts/release.sh --recover",
                        )
                    )
    nvattest_dir = entries.get("nvattest")
    if nvattest_dir is not None:
        nvattest_entry = nvattest_dir.lstat()
        if stat.S_ISLNK(nvattest_entry.st_mode) or not stat.S_ISDIR(
            nvattest_entry.st_mode
        ):
            failures.append(
                _failure(
                    "release nvattest entry is not an owned directory",
                    expected="non-symlink nvattest directory",
                    actual="non-directory",
                    repair="bash scripts/release.sh --recover",
                )
            )
        else:
            nvattest_entries = {path.name: path for path in nvattest_dir.iterdir()}
            expected_nvattest = {f"{target}.json" for target in PROOF_TARGETS}
            if set(nvattest_entries) != expected_nvattest:
                failures.append(
                    _failure(
                        "release nvattest inventory is not exact",
                        expected=", ".join(sorted(expected_nvattest)),
                        actual=", ".join(sorted(nvattest_entries)) or "<empty>",
                        repair="bash scripts/release.sh --recover",
                    )
                )
            for receipt_name, receipt_path in sorted(nvattest_entries.items()):
                receipt_entry = receipt_path.lstat()
                if not stat.S_ISREG(receipt_entry.st_mode):
                    failures.append(
                        _failure(
                            "release nvattest receipt is not a regular file",
                            expected=f"regular nvattest receipt {receipt_name}",
                            actual="non-regular",
                            repair="bash scripts/release.sh --recover",
                        )
                    )
    support_dir = entries.get("support")
    if support_dir is not None:
        support_entry = support_dir.lstat()
        if stat.S_ISLNK(support_entry.st_mode) or not stat.S_ISDIR(
            support_entry.st_mode
        ):
            failures.append(
                _failure(
                    "release support entry is not an owned directory",
                    expected="non-symlink support directory",
                    actual="non-directory",
                    repair="bash scripts/release.sh --recover",
                )
            )
        else:
            support_entries = {path.name: path for path in support_dir.iterdir()}
            if len(support_entries) != len(SUPPORT_DISTRIBUTION_NAMES):
                failures.append(
                    _failure(
                        "release support inventory is not structurally exact",
                        expected=(
                            f"{len(SUPPORT_DISTRIBUTION_NAMES)} retained support wheels"
                        ),
                        actual=str(len(support_entries)),
                        repair="bash scripts/release.sh --recover",
                    )
                )
            for wheel_name, wheel_path in sorted(support_entries.items()):
                wheel_entry = wheel_path.lstat()
                if (
                    stat.S_ISLNK(wheel_entry.st_mode)
                    or not stat.S_ISREG(wheel_entry.st_mode)
                    or not wheel_name.endswith(".whl")
                ):
                    actual = (
                        "non-.whl"
                        if stat.S_ISREG(wheel_entry.st_mode)
                        and not wheel_name.endswith(".whl")
                        else "non-file"
                    )
                    failures.append(
                        _failure(
                            "release support wheel is not a regular wheel file",
                            expected=f"regular .whl support file {wheel_name}",
                            actual=actual,
                            repair="bash scripts/release.sh --recover",
                        )
                    )
    prerequisite_path = entries.get(CORE_UNSUPPORTED_TOMBSTONE_RECORD)
    if publication_prerequisite_version is not None and prerequisite_path is not None:
        failures.extend(
            _validate_publication_prerequisite_record(
                prerequisite_path,
                version=publication_prerequisite_version,
            )
        )
    return failures


def _candidate_file_entries_from_dir(release_dir: Path) -> list[dict[str, Any]]:
    files: list[dict[str, Any]] = []
    for path in sorted(release_dir.iterdir(), key=lambda item: item.name):
        entry = path.lstat()
        if not stat.S_ISREG(entry.st_mode):
            continue
        sha256, byte_count = file_sha256_size(path)
        files.append({"name": path.name, "sha256": sha256, "bytes": byte_count})
    return files


def _validate_policy_payload(policy_run: Mapping[str, Any]) -> list[Failure]:
    failures: list[Failure] = []
    if (
        not isinstance(policy_run.get("advisory_source_id"), str)
        or not policy_run["advisory_source_id"]
    ):
        failures.append(
            _failure(
                "retained ledger advisory source ID is invalid",
                expected="non-empty public advisory source ID",
                actual=repr(policy_run.get("advisory_source_id")),
                repair="bash scripts/release.sh --recover",
            )
        )
    failures.extend(
        validate_snapshot_identity(
            "retained ledger policy_run",
            db_commit=policy_run.get("db_commit"),
            db_archive_sha256=policy_run.get("db_archive_sha256"),
        )
    )
    snapshot = policy_run.get("db_snapshot_basename")
    if not _safe_retained_basename(snapshot):
        failures.append(
            _failure(
                "retained ledger db snapshot basename is invalid",
                expected="safe snapshot directory basename",
                actual=repr(snapshot),
                repair="bash scripts/release.sh --recover",
            )
        )
    advisory_count = policy_run.get("advisory_count")
    if type(advisory_count) is not int or advisory_count <= 0:
        failures.append(
            _failure(
                "retained ledger advisory count is invalid",
                expected="positive integer advisory count",
                actual=repr(advisory_count),
                repair="bash scripts/release.sh --recover",
            )
        )
    for key in (
        "advisory_acquired_at",
        "db_commit_timestamp",
        "policy_checked_at",
    ):
        value = policy_run.get(key)
        if not is_normalized_utc_timestamp(value):
            failures.append(
                _failure(
                    f"retained ledger {key} is invalid",
                    expected="RFC3339 UTC timestamp normalized with Z",
                    actual=repr(value),
                    repair="bash scripts/release.sh --recover",
                )
            )
    if policy_run.get("result") != "pass":
        failures.append(
            _failure(
                "retained ledger policy result is invalid",
                expected="pass",
                actual=str(policy_run.get("result")),
                repair="bash scripts/release.sh --recover",
            )
        )
    return failures


def _validate_native_summary(
    release_dir: Path, ledger: Mapping[str, Any]
) -> list[Failure]:
    failures: list[Failure] = []
    summary = ledger.get("native_summary")
    members = ledger.get("native_members")
    if not isinstance(summary, Mapping) or not isinstance(members, Mapping):
        return [
            _failure(
                "retained ledger native summary is invalid",
                expected="native_summary and native_members objects",
                actual="missing or malformed",
                repair="bash scripts/release.sh --recover",
            )
        ]
    macos_members = members.get("macos-arm64")
    if not isinstance(macos_members, Mapping):
        return [
            _failure(
                "retained ledger macOS native members are invalid",
                expected="macos-arm64 native member map",
                actual=type(macos_members).__name__,
                repair="bash scripts/release.sh --recover",
            )
        ]
    summary_member_expectations = {
        "macos_root_helper": "parakeet-helper",
        "macos_core_script": "solstone-core",
    }
    for summary_key, member_key in summary_member_expectations.items():
        item = summary.get(summary_key)
        expected_member = macos_members.get(member_key)
        if not isinstance(item, Mapping) or not isinstance(expected_member, Mapping):
            failures.append(
                _failure(
                    "retained ledger native summary member is invalid",
                    expected=f"{summary_key} summary and {member_key} member",
                    actual=summary_key,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if item.get("member") != expected_member:
            failures.append(
                _failure(
                    "retained ledger native summary disagrees with native member map",
                    expected=repr(expected_member),
                    actual=repr(item.get("member")),
                    repair="bash scripts/release.sh --recover",
                )
            )
        wheel = item.get("wheel")
        if not isinstance(wheel, Mapping) or not isinstance(wheel.get("name"), str):
            failures.append(
                _failure(
                    "retained ledger native summary wheel is invalid",
                    expected=f"{summary_key} wheel name",
                    actual=repr(wheel),
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        try:
            expected_sha256, expected_bytes = file_sha256_size(
                release_dir / wheel["name"]
            )
        except OSError as exc:
            failures.append(
                _failure(
                    "retained ledger native summary wheel could not be read",
                    expected="final wheel named by native summary",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            )
            continue
        if (
            wheel.get("sha256") != expected_sha256
            or wheel.get("bytes") != expected_bytes
        ):
            failures.append(
                _failure(
                    "retained ledger native summary wheel disagrees with final bytes",
                    expected=f"{expected_sha256}/{expected_bytes}",
                    actual=repr(wheel),
                    repair="bash scripts/release.sh --recover",
                )
            )
    return failures


def _validate_deep_ledger_binding(
    *,
    root: Path,
    version: str,
    source_commit: str,
    expected_lock_sha256: str,
    release_dir: Path,
    evidence_dir: Path,
    ledger_path: Path,
    ledger: Mapping[str, Any],
    policy_run: PolicyRun | None = None,
    check_local_models_version: bool = True,
    validate_current_release_metadata: bool = True,
) -> list[Failure]:
    failures: list[Failure] = []
    try:
        if ledger_path.read_bytes() != canonical_json_bytes(ledger):
            failures.append(
                _failure(
                    "retained ledger bytes are not canonical JSON",
                    expected="canonical_json_bytes(ledger)",
                    actual="ledger bytes differ",
                    repair="bash scripts/release.sh --recover",
                )
            )
    except OSError as exc:
        failures.append(
            _failure(
                "retained ledger could not be read",
                expected="readable ledger.json",
                actual=type(exc).__name__,
                repair="bash scripts/release.sh --recover",
            )
        )
    expected_scalars = {
        "kind": "solstone-release-ledger",
        "product": "solstone",
        "version": version,
        "source_commit": source_commit,
        "core_lock_sha256": expected_lock_sha256,
    }
    for key, expected in expected_scalars.items():
        if ledger.get(key) != expected:
            failures.append(
                _failure(
                    f"retained ledger {key} is not bound to candidate",
                    expected=str(expected),
                    actual=str(ledger.get(key)),
                    repair="bash scripts/release.sh --recover",
                )
            )
    candidate_files = _candidate_file_entries_from_dir(release_dir)
    candidate = ledger.get("candidate")
    if not isinstance(candidate, Mapping):
        failures.append(
            _failure(
                "retained ledger candidate binding is invalid",
                expected="candidate object",
                actual=type(candidate).__name__,
                repair="bash scripts/release.sh --recover",
            )
        )
    else:
        package_file_count = sum(
            1
            for item in candidate_files
            if not item["name"].endswith(".rust-release-manifest.json")
        )
        manifest_file_count = len(candidate_files) - package_file_count
        expected_candidate = {
            "path": CANDIDATE,
            "file_count": len(candidate_files),
            "package_file_count": package_file_count,
            "manifest_file_count": manifest_file_count,
            "candidate_digest": candidate_digest(release_dir),
            "files": candidate_files,
        }
        if candidate != expected_candidate:
            failures.append(
                _failure(
                    "retained ledger candidate inventory disagrees with final payload",
                    expected="candidate names, counts, bytes, hashes, digest",
                    actual="candidate object differs",
                    repair="bash scripts/release.sh --recover",
                )
            )
    if validate_current_release_metadata:
        expected_targets = [
            {"lane": lane, "artifact": artifact, **target}
            for artifact, (lane, target) in sorted(rust_artifact_targets().items())
        ]
        if ledger.get("rust_targets") != expected_targets:
            failures.append(
                _failure(
                    "retained ledger Rust targets are invalid",
                    expected=repr(expected_targets),
                    actual=repr(ledger.get("rust_targets")),
                    repair="bash scripts/release.sh --recover",
                )
            )
    policy_payload = ledger.get("policy_run")
    if isinstance(policy_payload, Mapping):
        failures.extend(_validate_policy_payload(policy_payload))
        if policy_run is not None and policy_payload != {
            "advisory_source_id": policy_run.advisory_source_id,
            "db_snapshot_basename": policy_run.db_snapshot_basename,
            "db_commit": policy_run.db_commit,
            "db_archive_sha256": policy_run.db_archive_sha256,
            "advisory_count": policy_run.advisory_count,
            "advisory_acquired_at": policy_run.advisory_acquired_at,
            "db_commit_timestamp": policy_run.db_commit_timestamp,
            "policy_checked_at": policy_run.policy_checked_at,
            "result": policy_run.result,
        }:
            failures.append(
                _failure(
                    "retained ledger policy run disagrees with finalized policy cohort",
                    expected="policy run used for candidate finalization",
                    actual="policy_run differs",
                    repair="bash scripts/release.sh --recover",
                )
            )
    if ledger.get("proofs") != {"expected_targets": list(PROOF_TARGETS)}:
        failures.append(
            _failure(
                "retained ledger expected proof IDs are invalid",
                expected=", ".join(PROOF_TARGETS),
                actual=repr(ledger.get("proofs")),
                repair="bash scripts/release.sh --recover",
            )
        )
    if ledger.get("redaction") != {"validator": "recursive-key-value-public-evidence"}:
        failures.append(
            _failure(
                "retained ledger redaction marker is invalid",
                expected="recursive-key-value-public-evidence",
                actual=repr(ledger.get("redaction")),
                repair="bash scripts/release.sh --recover",
            )
        )
    if validate_current_release_metadata:
        model_failures = _validate_models_binding(
            root,
            release_dir,
            ledger,
            check_local_version=check_local_models_version,
        )
        failures.extend(model_failures)
    native_member_failures = validate_native_members_against_release_dir(
        release_dir, ledger
    )
    failures.extend(native_member_failures)
    failures.extend(_validate_native_summary(release_dir, ledger))
    failures.extend(validate_public_evidence_tree("ledger", ledger))
    return failures


def _cleanup_owned_cohorts(paths: Sequence[Path]) -> list[Failure]:
    failures: list[Failure] = []
    for path in paths:
        failures.extend(_remove_owned_path(path, label=path.name))
    return failures


def _aggregate_finalization_error(
    error: BaseException, cleanup_failures: Sequence[Failure]
) -> DriverError:
    if isinstance(error, DriverError):
        failures = [*error.failures, *cleanup_failures]
    else:
        failures = [
            _failure(
                "release candidate finalization transaction failed",
                expected="payload and evidence pair-promoted",
                actual=type(error).__name__,
                repair="bash scripts/release.sh --candidate",
            ),
            *cleanup_failures,
        ]
    return DriverError(failures)


def _pair_promote_payload_and_evidence(
    *,
    payload_staging: Path,
    ready_path: Path,
    evidence_staging: Path,
    evidence_dir: Path,
    include_models: bool,
    hook: Callable[[str], None],
) -> None:
    promoted_payload = False
    promoted_evidence = False
    try:
        current_schema_version, current_schema = current_retained_ledger_schema()
        failures = [
            *_validate_flat_payload_inventory(
                payload_staging, include_models=include_models
            ),
            *_validate_evidence_inventory(
                evidence_staging,
                schema_version=current_schema_version,
                schema=current_schema,
            ),
        ]
        for final_path, label in (
            (ready_path, "final payload"),
            (evidence_dir, "final evidence"),
        ):
            if not _is_missing(final_path):
                failures.append(
                    _failure(
                        "release finalization target already exists",
                        expected=f"absent {label} path",
                        actual=final_path.name,
                        repair="bash scripts/release.sh --candidate",
                    )
                )
        if failures:
            raise DriverError(failures)
        os.rename(payload_staging, ready_path)
        promoted_payload = True
        hook("after-payload-rename")
        hook("between-renames")
        os.rename(evidence_staging, evidence_dir)
        promoted_evidence = True
        hook("after-evidence-rename")
    except BaseException as exc:
        cleanup_targets = [
            ready_path if promoted_payload else payload_staging,
            evidence_dir if promoted_evidence else evidence_staging,
        ]
        cleanup_failures = _cleanup_owned_cohorts(cleanup_targets)
        raise _aggregate_finalization_error(exc, cleanup_failures) from None


def _proof_hashes(
    proofs_dir: Path,
    *,
    ledger: Mapping[str, Any],
    digest: str,
    ledger_sha256: str,
    release_dir: Path,
    version: str,
) -> dict[str, str]:
    hashes: dict[str, str] = {}
    for target in PROOF_TARGETS:
        path = proofs_dir / f"{target}.json"
        if not path.is_file() or path.is_symlink():
            raise DriverError(
                [
                    _failure(
                        "release proof is missing",
                        expected=f"{target} proof",
                        actual="missing",
                        repair="bash scripts/release.sh --recover",
                    )
                ]
            )
        data = path.read_bytes()
        failures = validate_install_proof_bytes(
            data,
            target=target,
            version=version,
            source_commit=str(ledger.get("source_commit")),
            core_lock_sha256=str(ledger.get("core_lock_sha256")),
            candidate_digest=digest,
            ledger_sha256=ledger_sha256,
            candidate_dir=release_dir,
            ledger_payload=ledger,
        )
        if failures:
            raise DriverError(failures)
        proof = json.loads(data.decode("utf-8"))
        binding_failures = _validate_proof_binding(
            proof,
            target=target,
            ledger=ledger,
            digest=digest,
            ledger_sha256=ledger_sha256,
            release_dir=release_dir,
        )
        if binding_failures:
            raise DriverError(binding_failures)
        hashes[target] = file_sha256_size(path)[0]
    extras = sorted(
        path.name
        for path in proofs_dir.glob("*.json")
        if path.stem not in PROOF_TARGETS
    )
    if extras:
        raise DriverError(
            [
                _failure(
                    "release proof set has extra targets",
                    expected=", ".join(PROOF_TARGETS),
                    actual=", ".join(extras),
                    repair="bash scripts/release.sh --recover",
                )
            ]
        )
    return hashes


def _report(
    *,
    heading: str,
    root: Path,
    version: str,
    source_commit: str,
    expected_lock_sha256: str,
    release_dir: Path,
    evidence_dir: Path,
    policy_run: PolicyRun | None = None,
    check_local_models_version: bool = True,
    validate_current_release_metadata: bool = True,
    allow_publication_prerequisite: bool = False,
) -> CandidateReport:
    ledger_path = evidence_dir / "ledger.json"
    try:
        ledger = read_retained_ledger(ledger_path)
    except OSError as exc:
        raise DriverError(
            [
                _failure(
                    "retained ledger could not be read",
                    expected="readable retained ledger.json",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            ]
        ) from None
    except LedgerError as exc:
        raise DriverError(exc.failures) from None
    schema_version, schema = _resolve_driver_retained_ledger_schema(ledger)
    inventory_failures = _validate_evidence_inventory(
        evidence_dir,
        schema_version=schema_version,
        schema=schema,
        publication_prerequisite_version=(
            version if allow_publication_prerequisite else None
        ),
    )
    if inventory_failures:
        raise DriverError(inventory_failures)
    models = ledger.get("models")
    include_models = isinstance(models, Mapping) and models.get("decision") == "include"
    if validate_current_release_metadata:
        payload_failures = _validate_flat_payload_inventory(
            release_dir, include_models=include_models
        )
        payload_failures.extend(
            validate_release_dir(release_dir, expected_source_commit=source_commit)
        )
    else:
        expected_names, payload_failures = _ledger_candidate_file_names(ledger)
        payload_failures.extend(
            _validate_flat_payload_inventory(release_dir, expected_names=expected_names)
        )
        payload_failures.extend(_validate_retained_manifest_files(release_dir, ledger))
    if payload_failures:
        raise DriverError(payload_failures)
    digest = candidate_digest(release_dir)
    deep_failures = _validate_deep_ledger_binding(
        root=root,
        version=version,
        source_commit=source_commit,
        expected_lock_sha256=expected_lock_sha256,
        release_dir=release_dir,
        evidence_dir=evidence_dir,
        ledger_path=ledger_path,
        ledger=ledger,
        policy_run=policy_run,
        check_local_models_version=check_local_models_version,
        validate_current_release_metadata=validate_current_release_metadata,
    )
    if ledger["candidate"]["candidate_digest"] != digest:
        deep_failures.append(
            _failure(
                "candidate digest does not match retained ledger",
                expected=str(ledger["candidate"]["candidate_digest"]),
                actual=digest,
                repair="bash scripts/release.sh --recover",
            )
        )
    if deep_failures:
        raise DriverError(deep_failures)
    ledger_sha256 = file_sha256_size(ledger_path)[0]
    proof_hashes = _proof_hashes(
        evidence_dir / "proofs",
        ledger=ledger,
        digest=digest,
        ledger_sha256=ledger_sha256,
        release_dir=release_dir,
        version=version,
    )
    if retained_ledger_schema_declares_nvattest(schema):
        nvattest_hashes = _validate_retained_nvattest_binding(
            evidence_dir=evidence_dir,
            ledger=ledger,
            digest=digest,
            ledger_sha256=ledger_sha256,
            release_dir=release_dir,
            version=version,
        )
    else:
        nvattest_hashes = {}
    return CandidateReport(
        heading=heading,
        version=version,
        release_dir=release_dir,
        evidence_dir=evidence_dir,
        payload_files=sum(1 for path in release_dir.iterdir() if path.is_file()),
        candidate_digest=digest,
        ledger_sha256=ledger_sha256,
        proof_sha256=proof_hashes,
        nvattest_sha256=nvattest_hashes,
        bundle_digest=bundle_digest(
            digest,
            ledger_sha256,
            proof_hashes,
            nvattest_hashes,
        ),
    )


def _inventory_entry(path: Path, *, name: str) -> dict[str, Any]:
    sha256, byte_count = file_sha256_size(path)
    return {"name": name, "sha256": sha256, "bytes": byte_count}


def _payload_report_inventory(release_dir: Path) -> list[dict[str, Any]]:
    return [
        _inventory_entry(path, name=path.name)
        for path in sorted(release_dir.iterdir(), key=lambda item: item.name)
        if stat.S_ISREG(path.lstat().st_mode)
    ]


def _evidence_report_inventory(
    evidence_dir: Path,
    *,
    schema: RetainedLedgerSchema,
) -> list[dict[str, Any]]:
    entries = [
        _inventory_entry(evidence_dir / "ledger.json", name="ledger.json"),
    ]
    proofs_dir = evidence_dir / "proofs"
    entries.extend(
        _inventory_entry(path, name=f"proofs/{path.name}")
        for path in sorted(proofs_dir.iterdir(), key=lambda item: item.name)
    )
    if not retained_ledger_schema_declares_nvattest(schema):
        return sorted(entries, key=lambda item: item["name"])
    nvattest_dir = evidence_dir / "nvattest"
    entries.extend(
        _inventory_entry(path, name=f"nvattest/{path.name}")
        for path in sorted(nvattest_dir.iterdir(), key=lambda item: item.name)
    )
    support_dir = evidence_dir / "support"
    entries.extend(
        _inventory_entry(path, name=f"support/{path.name}")
        for path in sorted(support_dir.iterdir(), key=lambda item: item.name)
    )
    return sorted(entries, key=lambda item: item["name"])


def _proof_report_inventory(evidence_dir: Path) -> dict[str, dict[str, Any]]:
    proofs_dir = evidence_dir / "proofs"
    return {
        target: _inventory_entry(proofs_dir / f"{target}.json", name=f"{target}.json")
        for target in sorted(PROOF_TARGETS)
    }


def _nvattest_report_inventory(
    evidence_dir: Path,
    *,
    schema: RetainedLedgerSchema,
) -> dict[str, dict[str, Any]]:
    if not retained_ledger_schema_declares_nvattest(schema):
        return {}
    nvattest_dir = evidence_dir / "nvattest"
    return {
        target: _inventory_entry(nvattest_dir / f"{target}.json", name=f"{target}.json")
        for target in sorted(PROOF_TARGETS)
    }


def _support_report_inventory(
    evidence_dir: Path,
    *,
    schema: RetainedLedgerSchema,
) -> list[dict[str, Any]]:
    if not retained_ledger_schema_declares_nvattest(schema):
        return []
    support_dir = evidence_dir / "support"
    return [
        _inventory_entry(path, name=path.name)
        for path in sorted(support_dir.iterdir(), key=lambda item: item.name)
    ]


def _publication_prerequisite_report_inventory(
    evidence_dir: Path,
) -> list[dict[str, Any]]:
    path = evidence_dir / CORE_UNSUPPORTED_TOMBSTONE_RECORD
    try:
        path.lstat()
    except FileNotFoundError:
        return []
    return [_inventory_entry(path, name=path.name)]


def format_report(report: CandidateReport) -> str:
    try:
        ledger = read_retained_ledger(report.evidence_dir / "ledger.json")
    except LedgerError as exc:
        raise DriverError(exc.failures) from None
    schema_version, schema = _resolve_driver_retained_ledger_schema(ledger)
    payload = {
        "schema_version": 1,
        "kind": "solstone-release-candidate-report",
        "verdict": report.heading,
        "publication_authorization": "local candidate evidence only; not publication authorization",
        "retained_ledger_schema_version": schema_version,
        "version": report.version,
        "release_dir": CANDIDATE,
        "evidence_dir": "EVIDENCE",
        "payload_files": report.payload_files,
        "candidate_digest": report.candidate_digest,
        "ledger_sha256": report.ledger_sha256,
        "bundle_digest": report.bundle_digest,
        "payload_inventory": _payload_report_inventory(report.release_dir),
        "evidence_inventory": _evidence_report_inventory(
            report.evidence_dir,
            schema=schema,
        ),
        "proof_inventory": _proof_report_inventory(report.evidence_dir),
        "proof_sha256": {
            target: report.proof_sha256[target]
            for target in sorted(report.proof_sha256)
        },
        "nvattest_sha256": {
            target: report.nvattest_sha256[target]
            for target in sorted(report.nvattest_sha256)
        },
        "publication_prerequisite_inventory": (
            _publication_prerequisite_report_inventory(report.evidence_dir)
        ),
    }
    if retained_ledger_schema_declares_nvattest(schema):
        payload["nvattest_inventory"] = _nvattest_report_inventory(
            report.evidence_dir,
            schema=schema,
        )
        payload["support_inventory"] = _support_report_inventory(
            report.evidence_dir,
            schema=schema,
        )
    return canonical_json_bytes(payload).decode("utf-8")


def default_dry_run_plan(env: Mapping[str, str]) -> DryRunPlan:
    include_models = _models_include(env)
    return DryRunPlan(
        models_decision="include" if include_models else "exclude",
        artifacts=tuple(sorted(expected_package_names(include_models=include_models))),
        tool_evidence={
            lane: fixture_lane_tool_evidence(lane)
            for lane in ("source", "linux-x86_64-musl", "linux-aarch64-musl")
        },
        linux_maturin_args={
            "x86_64-unknown-linux-musl": CORE_X86_64_MATURIN_ARGS,
            "aarch64-unknown-linux-musl": CORE_AARCH64_MATURIN_ARGS,
        },
        publication_lockout={
            "default": True,
            "--test": True,
            "make release": True,
            "make release-test": True,
        },
    )


def validate_dry_run_plan(plan: DryRunPlan) -> list[Failure]:
    failures: list[Failure] = []
    if plan.models_decision not in {"include", "exclude"}:
        failures.append(
            _failure(
                "dry-run model decision is invalid",
                expected="include or exclude",
                actual=plan.models_decision,
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
        include_models = False
    else:
        include_models = plan.models_decision == "include"
    expected_artifacts = set(expected_package_names(include_models=include_models))
    actual_artifacts = set(plan.artifacts)
    if actual_artifacts != expected_artifacts:
        failures.append(
            _failure(
                "dry-run artifact plan is invalid",
                expected=", ".join(sorted(expected_artifacts)),
                actual=", ".join(sorted(actual_artifacts)) or "<empty>",
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
    linux_core_wheels = [
        name
        for name in actual_artifacts
        if name.startswith("solstone_core-")
        and ("manylinux2014_x86_64" in name or "manylinux2014_aarch64" in name)
    ]
    if len(linux_core_wheels) != 2:
        failures.append(
            _failure(
                "dry-run Linux core wheel plan is incomplete",
                expected="x86_64 and aarch64 musl core wheels",
                actual=", ".join(sorted(linux_core_wheels)) or "<empty>",
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
    expected_tool_lanes = {"source", "linux-x86_64-musl", "linux-aarch64-musl"}
    if set(plan.tool_evidence) != expected_tool_lanes:
        failures.append(
            _failure(
                "dry-run tool evidence lanes are invalid",
                expected=", ".join(sorted(expected_tool_lanes)),
                actual=", ".join(sorted(str(key) for key in plan.tool_evidence))
                or "<empty>",
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
    for lane in sorted(set(plan.tool_evidence) & expected_tool_lanes):
        failures.extend(check_lane_tool_evidence(lane, plan.tool_evidence[lane]))
    expected_args = {
        "x86_64-unknown-linux-musl": CORE_X86_64_MATURIN_ARGS,
        "aarch64-unknown-linux-musl": CORE_AARCH64_MATURIN_ARGS,
    }
    if set(plan.linux_maturin_args) != set(expected_args):
        failures.append(
            _failure(
                "dry-run Linux build-arg targets are invalid",
                expected=", ".join(sorted(expected_args)),
                actual=", ".join(sorted(str(key) for key in plan.linux_maturin_args))
                or "<empty>",
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
    for target, expected in sorted(expected_args.items()):
        actual = plan.linux_maturin_args.get(target, "")
        if actual != expected:
            failures.append(
                _failure(
                    "dry-run Linux build arguments are invalid",
                    expected=expected,
                    actual=actual or "<missing>",
                    repair="bash scripts/release.sh --dry-run-linux",
                )
            )
        failures.extend(validate_linux_maturin_args(actual, target=target))
    expected_lockouts = {
        "default": True,
        "--test": True,
        "make release": True,
        "make release-test": True,
    }
    if dict(plan.publication_lockout) != expected_lockouts:
        failures.append(
            _failure(
                "dry-run publication lockout plan is invalid",
                expected=repr(expected_lockouts),
                actual=repr(dict(plan.publication_lockout)),
                repair="bash scripts/release.sh --dry-run-linux",
            )
        )
    return failures


def run_candidate(
    root: Path,
    env: Mapping[str, str],
    services: CandidateServices | None = None,
) -> CandidateReport:
    svc = services or default_services(env)
    version = _project_version(root)
    expected_commit = _expected_commit(env)
    include_models = _models_include(env)
    models_decision = "include" if include_models else "exclude"
    models_package_version = _local_models_package_version(root)
    expected_lock = svc.core_lock_sha256(root)
    _assert_clean_identity(
        root,
        expected_commit=expected_commit,
        expected_lock_sha256=expected_lock,
        services=svc,
    )
    retained_presence = _retained_candidate_presence(root, version)
    if retained_presence.failures:
        raise DriverError(retained_presence.failures)
    if retained_presence.present_paths:
        tag_lookup = svc.git_tag_commit(root, version)
        retained_failures = _retained_candidate_authorization_failures(
            retained_presence,
            tag_lookup,
            env,
            version,
        )
        if retained_failures:
            raise DriverError(retained_failures)
    svc.clean_outputs(root, version)
    policy_run = svc.prepare_policy(root, env)
    coordinator_tool_evidence = _validate_coordinator_tool_evidence(
        svc.coordinator_tool_evidence()
    )
    svc.build_local_dist(root, include_models)
    transfer_dir = root / "target" / "release-transfer" / version
    source_bundle_path = _source_bundle_staging_path(root, version)
    try:
        source_bundle = svc.create_source_bundle(
            root, expected_commit, source_bundle_path
        )
        host_result = svc.build_host(source_bundle, expected_commit, transfer_dir)
        native_records = _native_record_payloads(
            host_result,
            source_commit=expected_commit,
            core_lock_sha256=expected_lock,
        )
        full_tool_evidence = _validated_full_tool_evidence(
            coordinator_tool_evidence,
            host_result,
            native_records,
        )
        lane_evidence = _lane_evidence_from_full_tool_evidence(
            full_tool_evidence,
            policy_run=policy_run,
        )
        _copy_macos_wheels(host_result, root / "dist")
        _revalidate_macos_wheels(
            root / "dist",
            native_records,
            source_commit=expected_commit,
            core_lock_sha256=expected_lock,
        )
    finally:
        svc.cleanup_transients(
            (transfer_dir, source_bundle_path, _zig_cache_root(root))
        )
    _assert_clean_identity(
        root,
        expected_commit=expected_commit,
        expected_lock_sha256=expected_lock,
        services=svc,
    )
    ready_path = root / "dist" / RESERVED_CANDIDATE_DIRNAME / version
    ready_path.parent.mkdir(parents=True, exist_ok=True)
    payload_staging = ready_path.parent / f"{version}.payload-staging"
    evidence_root = root / "target" / "release-evidence"
    evidence_dir = evidence_root / version
    evidence_staging = evidence_root / f"{version}.staging"

    def post_promote_hook(release_dir: Path) -> None:
        _validate_candidate_wheel_contents(
            release_dir,
            models_decision=models_decision,
        )
        _revalidate_macos_wheels(
            release_dir,
            native_records,
            source_commit=expected_commit,
            core_lock_sha256=expected_lock,
        )
        support_distributions, support_paths = _materialize_verified_support_wheels(
            root=root,
            support_dir=evidence_staging / "support",
            services=svc,
        )
        challenge = svc.challenge_factory()
        challenge_failures = _validate_nvattest_challenge(challenge)
        if challenge_failures:
            raise DriverError(challenge_failures)
        authority_bytes, authority_failures = _extract_nvattest_authority_bytes(
            release_dir,
            _candidate_names_from_dir(release_dir),
        )
        if authority_failures or authority_bytes is None:
            raise DriverError(authority_failures)
        nvattest_payload = _nvattest_ledger_payload(
            challenge=challenge,
            authority_bytes=authority_bytes,
            support_distributions=support_distributions,
        )
        ledger_path = write_ledger(
            evidence_root=evidence_root,
            version=version,
            source_commit=expected_commit,
            release_dir=release_dir,
            core_lock_path=root / "core" / "Cargo.lock",
            tool_evidence={
                lane: full_tool_evidence[lane] for lane in sorted(full_tool_evidence)
            },
            policy_run=policy_run,
            native_records=native_records,
            models={
                "decision": models_decision,
                "package_version": models_package_version,
            },
            nvattest=nvattest_payload,
            output_dir=evidence_staging,
        )
        ledger_sha256 = file_sha256_size(ledger_path)[0]
        ledger_payload = read_retained_ledger(ledger_path)
        candidate_paths = sorted(
            path for path in release_dir.iterdir() if path.is_file()
        )
        proofs_dir = evidence_staging / "proofs"
        for target in PROOF_TARGETS:
            svc.run_target_proofs(
                target=target,
                version=version,
                source_commit=expected_commit,
                core_lock_sha256=expected_lock,
                candidate_digest=ledger_payload["candidate"]["candidate_digest"],
                ledger_sha256=ledger_sha256,
                candidate_dir=release_dir,
                candidate_paths=candidate_paths,
                ledger_payload=ledger_payload,
                challenge=challenge,
                support_wheel_paths=support_paths,
                canonical_authority_bytes=authority_bytes,
                output_path=proofs_dir / f"{target}.json",
                nvattest_output_path=(evidence_staging / "nvattest" / f"{target}.json"),
            )
        failures = validate_public_evidence_tree("ledger", ledger_payload)
        if failures:
            raise DriverError(failures)

    try:
        failures = build_and_promote_candidate(
            root / "dist",
            payload_staging,
            source_commit=expected_commit,
            evidence_by_lane=lane_evidence,
            include_models=include_models,
            cargo_lock_path=root / "core" / "Cargo.lock",
            _post_promote_hook=post_promote_hook,
        )
    except BaseException as exc:
        cleanup_failures = _cleanup_owned_cohorts(
            (payload_staging, evidence_staging, ready_path, evidence_dir)
        )
        raise _aggregate_finalization_error(exc, cleanup_failures) from None
    if failures:
        cleanup_failures = _cleanup_owned_cohorts((payload_staging, evidence_staging))
        raise DriverError([*failures, *cleanup_failures])
    try:
        _pair_promote_payload_and_evidence(
            payload_staging=payload_staging,
            ready_path=ready_path,
            evidence_staging=evidence_staging,
            evidence_dir=evidence_dir,
            include_models=include_models,
            hook=svc.transaction_hook,
        )
        _assert_clean_identity(
            root,
            expected_commit=expected_commit,
            expected_lock_sha256=expected_lock,
            services=svc,
        )
        report = _report(
            heading="candidate-proven",
            root=root,
            version=version,
            source_commit=expected_commit,
            expected_lock_sha256=expected_lock,
            release_dir=ready_path,
            evidence_dir=evidence_dir,
            policy_run=policy_run,
        )
    except BaseException as exc:
        cleanup_failures = _cleanup_owned_cohorts((ready_path, evidence_dir))
        raise _aggregate_finalization_error(exc, cleanup_failures) from None
    return report


def run_recover(
    root: Path,
    *,
    version: str,
    source_commit: str,
) -> CandidateReport:
    if not version:
        raise DriverError(
            [
                _failure(
                    "retained release version selector is missing",
                    expected="explicit retained release version",
                    actual="<missing>",
                    repair="bash scripts/release.sh --recover",
                )
            ]
        )
    if not _safe_retained_basename(version):
        raise DriverError(
            [
                _failure(
                    "retained release version selector is unsafe",
                    expected="safe retained release version basename",
                    actual=repr(version),
                    repair="bash scripts/release.sh --recover",
                )
            ]
        )
    if not SOURCE_COMMIT_RE.fullmatch(source_commit):
        raise DriverError(
            [
                _failure(
                    "retained release source selector is invalid",
                    expected="40 or 64 lowercase hexadecimal characters",
                    actual=source_commit or "<missing>",
                    repair="bash scripts/release.sh --recover",
                )
            ]
        )
    release_dir = root / "dist" / RESERVED_CANDIDATE_DIRNAME / version
    evidence_dir = root / "target" / "release-evidence" / version
    try:
        ledger = read_retained_ledger(evidence_dir / "ledger.json")
    except OSError as exc:
        raise DriverError(
            [
                _failure(
                    "retained ledger could not be read for selector",
                    expected="retained ledger.json for explicit version",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --recover",
                )
            ]
        ) from None
    except LedgerError as exc:
        raise DriverError(exc.failures) from None
    _, schema = _resolve_driver_retained_ledger_schema(ledger)
    selector_failures: list[Failure] = []
    if ledger.get("version") != version:
        selector_failures.append(
            _failure(
                "retained ledger version does not match selector",
                expected=version,
                actual=str(ledger.get("version")),
                repair="bash scripts/release.sh --recover",
            )
        )
    if ledger.get("source_commit") != source_commit:
        selector_failures.append(
            _failure(
                "retained ledger source commit does not match selector",
                expected=source_commit,
                actual=str(ledger.get("source_commit")),
                repair="bash scripts/release.sh --recover",
            )
        )
    if selector_failures:
        raise DriverError(selector_failures)
    heading = (
        RETAINED_CANDIDATE_VALID_HEADING
        if retained_ledger_schema_declares_nvattest(schema)
        else RETAINED_PRE_NVATTEST_CANDIDATE_VALID_HEADING
    )
    return _report(
        heading=heading,
        root=root,
        version=version,
        source_commit=source_commit,
        expected_lock_sha256=str(ledger["core_lock_sha256"]),
        release_dir=release_dir,
        evidence_dir=evidence_dir,
        check_local_models_version=False,
        validate_current_release_metadata=False,
        allow_publication_prerequisite=True,
    )


def run_dry_run_linux(
    _root: Path,
    env: Mapping[str, str],
    *,
    plan: DryRunPlan | None = None,
) -> str:
    dry_run_plan = plan or default_dry_run_plan(env)
    failures = validate_dry_run_plan(dry_run_plan)
    if failures:
        raise DriverError(failures)
    return (
        "linux structural dry-run validated\n"
        "no release candidate, manifest, ledger, proof, or clean-source claim emitted"
    )


def main(
    argv: list[str] | None = None,
    env: Mapping[str, str] | None = None,
    services: CandidateServices | None = None,
) -> int:
    parser = argparse.ArgumentParser(
        description="Finalize solstone release candidates."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("candidate")
    recover_parser = subparsers.add_parser("recover")
    recover_parser.add_argument("--version", required=True)
    recover_parser.add_argument("--source-commit", required=True)
    subparsers.add_parser("dry-run-linux")
    args = parser.parse_args(argv)
    runtime_env = os.environ if env is None else env
    root = Path(__file__).resolve().parent.parent
    try:
        if args.command == "candidate":
            sys.stdout.write(format_report(run_candidate(root, runtime_env, services)))
        elif args.command == "recover":
            sys.stdout.write(
                format_report(
                    run_recover(
                        root,
                        version=args.version,
                        source_commit=args.source_commit,
                    )
                )
            )
        else:
            print(run_dry_run_linux(root, runtime_env))
    except Exception as exc:
        failures = _extract_failure_records(exc)
        if failures:
            _format_failures(failures)
        else:
            _format_failures(
                [
                    _failure(
                        "release candidate driver failed",
                        expected="successful candidate operation",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
