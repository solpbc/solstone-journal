#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Provider-neutral build-host channel for release candidates."""

from __future__ import annotations

import json
import shlex
import shutil
import stat
import subprocess
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from scripts.check_release_preflight import check_presign_lane_tool_evidence
from scripts.check_rust_release_manifest import (
    SOURCE_COMMIT_RE,
    Failure,
    canonical_json_bytes,
    expected_package_names,
)
from scripts.release_digest import file_sha256_size
from scripts.release_package_inventory import (
    load_release_package_inventory,
    macos_native_record_name,
    native_role,
    normalized_distribution,
)
from scripts.release_public_evidence import validate_public_evidence_tree

Runner = Callable[..., subprocess.CompletedProcess[str]]
IdFactory = Callable[[], str]
FileCopier = Callable[[Path, Path], object]


# Stable names retained for the three original native roles. The complete
# record set is derived by _expected_native_record_names().
MACOS_ROOT_RECORD = "macos-native-root.json"
MACOS_CORE_RECORD = "macos-native-core.json"
MACOS_SPEAKERS_ANALYZE_RECORD = "macos-native-speakers-analyze.json"


REQUEST_KEYS = frozenset(
    (
        "schema_version",
        "cohort_id",
        "expected_commit",
        "expected_version",
        "source_bundle",
        "expected_outputs",
        "paths",
    )
)
REQUEST_BUNDLE_KEYS = frozenset(("path", "source_commit", "sha256", "bytes"))
REQUEST_OUTPUT_KEYS = frozenset(
    (
        "macos_wheels",
        "native_records",
    )
)
REQUEST_PATH_KEYS = frozenset(("response", "output_dir"))
RESPONSE_KEYS = frozenset(
    (
        "schema_version",
        "cohort_id",
        "attestation",
        "tool_evidence",
        "macos_wheels",
        "native_records",
    )
)
ATTESTATION_KEYS = frozenset(
    ("source_commit", "clean_tree", "bundle_sha256", "bundle_bytes")
)


@dataclass(frozen=True)
class SourceBundle:
    path: Path
    source_commit: str
    sha256: str
    bytes: int


@dataclass(frozen=True)
class BuildHostResult:
    macos_wheels: tuple[Path, ...]
    native_records: tuple[Path, ...]
    tool_evidence: Mapping[str, str]


@dataclass(frozen=True)
class DirectoryIdentity:
    path: Path
    label: str
    st_dev: int
    st_ino: int
    mode: int


class BuildHostChannel(Protocol):
    def build_macos(
        self,
        *,
        source_bundle: SourceBundle,
        expected_commit: str,
        output_dir: Path,
    ) -> BuildHostResult: ...


class BuildHostError(RuntimeError):
    def __init__(self, failures: Sequence[Failure]) -> None:
        self.failures = tuple(failures)
        super().__init__("; ".join(failure.error for failure in self.failures))


def _failure(error: str, *, expected: str, actual: str, repair: str) -> Failure:
    return Failure(error=error, expected=expected, actual=actual, repair=repair)


def _run(
    runner: Runner,
    argv: Sequence[str],
    *,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    return runner(
        list(argv),
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )


def _single_trailing_lf(value: str) -> str:
    if value.endswith("\n"):
        return value[:-1]
    return value


def _git_stdout(runner: Runner, argv: Sequence[str], *, cwd: Path) -> str:
    result = _run(runner, argv, cwd=cwd)
    if result.returncode != 0:
        raise BuildHostError(
            [
                _failure(
                    "build-host git command failed",
                    expected="git exit 0",
                    actual=result.stderr.strip()
                    or result.stdout.strip()
                    or f"exit {result.returncode}",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return _single_trailing_lf(result.stdout)


def create_source_bundle(
    root: Path,
    *,
    expected_commit: str,
    output_path: Path,
    runner: Runner = subprocess.run,
) -> SourceBundle:
    if not SOURCE_COMMIT_RE.fullmatch(expected_commit):
        raise BuildHostError(
            [
                _failure(
                    "build-host source commit is invalid",
                    expected="40 or 64 lowercase hexadecimal characters",
                    actual=expected_commit,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    output_path.parent.mkdir(parents=True, exist_ok=True)
    result = _run(
        runner,
        ["git", "bundle", "create", str(output_path), "HEAD"],
        cwd=root,
    )
    if result.returncode != 0:
        raise BuildHostError(
            [
                _failure(
                    "build-host source bundle failed",
                    expected="git bundle exit 0",
                    actual=result.stderr.strip() or result.stdout.strip() or "failed",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    heads = _git_stdout(
        runner,
        ["git", "bundle", "list-heads", str(output_path)],
        cwd=root,
    )
    expected_heads = f"{expected_commit} HEAD"
    if heads != expected_heads:
        raise BuildHostError(
            [
                _failure(
                    "build-host source bundle HEAD is wrong",
                    expected=expected_heads,
                    actual=heads or "<empty>",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    verify = _run(runner, ["git", "bundle", "verify", str(output_path)], cwd=root)
    if verify.returncode != 0:
        raise BuildHostError(
            [
                _failure(
                    "build-host source bundle verification failed",
                    expected="git bundle verify exit 0",
                    actual=verify.stderr.strip()
                    or verify.stdout.strip()
                    or f"exit {verify.returncode}",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    sha256, byte_count = file_sha256_size(output_path)
    return SourceBundle(
        path=output_path,
        source_commit=expected_commit,
        sha256=sha256,
        bytes=byte_count,
    )


def _combine_failures(
    primary: BuildHostError | None, cleanup: BuildHostError | None
) -> BuildHostError | None:
    failures: list[Failure] = []
    if primary is not None:
        failures.extend(primary.failures)
    if cleanup is not None:
        failures.extend(cleanup.failures)
    return BuildHostError(failures) if failures else None


def _key_set_failures(
    label: str,
    payload: Mapping[str, object],
    expected: frozenset[str],
) -> list[Failure]:
    actual = frozenset(str(key) for key in payload)
    if actual == expected:
        return []
    return [
        _failure(
            f"{label} key set is invalid",
            expected=", ".join(sorted(expected)),
            actual=", ".join(sorted(actual)) or "<empty>",
            repair="bash scripts/release.sh --candidate",
        )
    ]


def _json_object(text: str) -> Mapping[str, object]:
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise BuildHostError(
            [
                _failure(
                    "build-host response is not JSON",
                    expected="JSON object with attestation and artifacts",
                    actual=str(exc),
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from exc
    if not isinstance(payload, Mapping):
        raise BuildHostError(
            [
                _failure(
                    "build-host response is not an object",
                    expected="JSON object",
                    actual=type(payload).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    return payload


def _safe_basename(name: object) -> str | None:
    if not isinstance(name, str):
        return None
    if not name or name in {".", ".."}:
        return None
    if Path(name).name != name or "/" in name or "\\" in name:
        return None
    return name


def _validate_cohort_id(cohort_id: str) -> None:
    if (
        len(cohort_id) != 32
        or cohort_id.lower() != cohort_id
        or any(char not in "0123456789abcdef" for char in cohort_id)
    ):
        raise BuildHostError(
            [
                _failure(
                    "build-host cohort id is invalid",
                    expected="32 lowercase hexadecimal characters",
                    actual=cohort_id,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )


def _regular_file_failure(label: str, path: Path) -> Failure | None:
    try:
        mode = path.lstat().st_mode
    except OSError as exc:
        return _failure(
            f"build-host {label} is not a regular file",
            expected="regular non-symlink file",
            actual=type(exc).__name__,
            repair="bash scripts/release.sh --candidate",
        )
    if stat.S_ISLNK(mode):
        return _failure(
            f"build-host {label} is not a regular file",
            expected="regular non-symlink file",
            actual="symlink",
            repair="bash scripts/release.sh --candidate",
        )
    if not stat.S_ISREG(mode):
        return _failure(
            f"build-host {label} is not a regular file",
            expected="regular non-symlink file",
            actual="non-regular",
            repair="bash scripts/release.sh --candidate",
        )
    return None


def _directory_identity_failure(identity: DirectoryIdentity) -> Failure | None:
    try:
        current = identity.path.lstat()
    except OSError as exc:
        return _failure(
            f"build-host {identity.label} directory identity changed",
            expected="same owned non-symlink directory",
            actual=type(exc).__name__,
            repair="bash scripts/release.sh --candidate",
        )
    if stat.S_ISLNK(current.st_mode):
        actual = "symlink"
    elif not stat.S_ISDIR(current.st_mode):
        actual = "non-directory"
    elif (
        current.st_dev != identity.st_dev
        or current.st_ino != identity.st_ino
        or current.st_mode != identity.mode
    ):
        actual = "different directory"
    else:
        return None
    return _failure(
        f"build-host {identity.label} directory identity changed",
        expected="same owned non-symlink directory",
        actual=actual,
        repair="bash scripts/release.sh --candidate",
    )


def _capture_directory_identity(path: Path, *, label: str) -> DirectoryIdentity:
    try:
        current = path.lstat()
    except OSError as exc:
        raise BuildHostError(
            [
                _failure(
                    f"build-host {label} directory identity could not be captured",
                    expected="owned non-symlink directory",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        ) from None
    if stat.S_ISLNK(current.st_mode):
        actual = "symlink"
    elif not stat.S_ISDIR(current.st_mode):
        actual = "non-directory"
    else:
        return DirectoryIdentity(
            path=path,
            label=label,
            st_dev=current.st_dev,
            st_ino=current.st_ino,
            mode=current.st_mode,
        )
    raise BuildHostError(
        [
            _failure(
                f"build-host {label} directory identity could not be captured",
                expected="owned non-symlink directory",
                actual=actual,
                repair="bash scripts/release.sh --candidate",
            )
        ]
    )


def _validate_directory_identity(identity: DirectoryIdentity) -> None:
    failure = _directory_identity_failure(identity)
    if failure is not None:
        raise BuildHostError([failure])


def _validate_directory_identities(
    identities: Sequence[DirectoryIdentity | None],
) -> None:
    failures = [
        failure
        for identity in identities
        if identity is not None
        if (failure := _directory_identity_failure(identity)) is not None
    ]
    if failures:
        raise BuildHostError(failures)


def _validate_regular_file(path: Path, *, label: str) -> None:
    failure = _regular_file_failure(label, path)
    if failure is not None:
        raise BuildHostError([failure])


def _validate_source_bundle(
    source_bundle: SourceBundle,
    *,
    requested_commit: str,
    label: str,
) -> None:
    failures: list[Failure] = []
    if source_bundle.source_commit != requested_commit:
        failures.append(
            _failure(
                f"build-host {label} source commit is wrong",
                expected=requested_commit,
                actual=source_bundle.source_commit,
                repair="bash scripts/release.sh --candidate",
            )
        )
    if not SOURCE_COMMIT_RE.fullmatch(source_bundle.source_commit):
        failures.append(
            _failure(
                f"build-host {label} source commit is invalid",
                expected="40 or 64 lowercase hexadecimal characters",
                actual=source_bundle.source_commit,
                repair="bash scripts/release.sh --candidate",
            )
        )
    regular_failure = _regular_file_failure(label, source_bundle.path)
    if regular_failure is not None:
        failures.append(regular_failure)
    else:
        try:
            actual_sha256, actual_bytes = file_sha256_size(source_bundle.path)
        except OSError as exc:
            failures.append(
                _failure(
                    f"build-host {label} could not be hashed",
                    expected="readable source bundle bytes",
                    actual=type(exc).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            )
        else:
            if actual_sha256 != source_bundle.sha256:
                failures.append(
                    _failure(
                        f"build-host {label} SHA-256 changed",
                        expected=source_bundle.sha256,
                        actual=actual_sha256,
                        repair="bash scripts/release.sh --candidate",
                    )
                )
            if actual_bytes != source_bundle.bytes:
                failures.append(
                    _failure(
                        f"build-host {label} byte length changed",
                        expected=str(source_bundle.bytes),
                        actual=str(actual_bytes),
                        repair="bash scripts/release.sh --candidate",
                    )
                )
    if failures:
        raise BuildHostError(failures)


def _wheel_role(name: str) -> str | None:
    return {
        wheel_name: role
        for role, wheel_name in _expected_macos_wheels_by_role().items()
    }.get(name)


def _expected_macos_wheels_by_role() -> dict[str, str]:
    inventory = load_release_package_inventory()
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


def _expected_macos_wheel_names() -> tuple[str, ...]:
    return tuple(_expected_macos_wheels_by_role().values())


def _expected_native_record_names() -> tuple[str, ...]:
    return tuple(
        macos_native_record_name(role) for role in _expected_macos_wheels_by_role()
    )


def _expected_release_version() -> str:
    expected_root = _expected_macos_wheels_by_role()["root"]
    return expected_root.removeprefix("solstone-").split("-", 1)[0]


def _validate_fresh_directory_path(path: Path, *, label: str) -> None:
    try:
        path.lstat()
    except FileNotFoundError:
        return
    if path.is_symlink():
        raise BuildHostError(
            [
                _failure(
                    f"build-host {label} directory is unsafe",
                    expected="fresh non-symlink directory path",
                    actual="symlink",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    if not path.is_dir():
        raise BuildHostError(
            [
                _failure(
                    f"build-host {label} directory is unsafe",
                    expected="fresh directory path",
                    actual="non-directory",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    if any(path.iterdir()):
        raise BuildHostError(
            [
                _failure(
                    f"build-host {label} directory is not empty",
                    expected="empty directory path",
                    actual="pre-existing entries",
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    raise BuildHostError(
        [
            _failure(
                f"build-host {label} directory already exists",
                expected="fresh directory path",
                actual="empty directory",
                repair="bash scripts/release.sh --candidate",
            )
        ]
    )


def _validate_attestation(
    payload: Mapping[str, object],
    *,
    expected_commit: str,
    source_bundle: SourceBundle,
) -> None:
    attestation = payload.get("attestation")
    if not isinstance(attestation, Mapping):
        raise BuildHostError(
            [
                _failure(
                    "build-host response attestation is invalid",
                    expected="attestation object",
                    actual=type(attestation).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    key_failures = _key_set_failures(
        "build-host response attestation",
        attestation,
        ATTESTATION_KEYS,
    )
    if key_failures:
        raise BuildHostError(key_failures)
    expected = {
        "source_commit": expected_commit,
        "clean_tree": True,
        "bundle_sha256": source_bundle.sha256,
        "bundle_bytes": source_bundle.bytes,
    }
    failures: list[Failure] = []
    for key, expected_value in expected.items():
        if attestation.get(key) != expected_value:
            failures.append(
                _failure(
                    f"build-host response attestation {key} is wrong",
                    expected=repr(expected_value),
                    actual=repr(attestation.get(key)),
                    repair="bash scripts/release.sh --candidate",
                )
            )
    if failures:
        raise BuildHostError(failures)


def _validate_macos_tool_evidence(payload: Mapping[str, object]) -> dict[str, str]:
    raw = payload.get("tool_evidence")
    if not isinstance(raw, Mapping):
        raise BuildHostError(
            [
                _failure(
                    "build-host macOS tool evidence is invalid",
                    expected="macOS release tool evidence object",
                    actual=type(raw).__name__,
                    repair="bash scripts/release.sh --candidate",
                )
            ]
        )
    evidence = {str(key): value for key, value in raw.items()}
    failures = check_presign_lane_tool_evidence("macos-arm64", evidence)  # type: ignore[arg-type]
    failures.extend(validate_public_evidence_tree("build_host.tool_evidence", evidence))
    if failures:
        raise BuildHostError(failures)
    return {key: str(value) for key, value in evidence.items()}


def _names_from_payload(
    payload: Mapping[str, object],
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    raw_wheels = payload.get("macos_wheels")
    raw_records = payload.get("native_records")
    failures: list[Failure] = []
    if not isinstance(raw_wheels, list):
        failures.append(
            _failure(
                "build-host macOS wheel list is invalid",
                expected="list of safe basenames",
                actual=type(raw_wheels).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        )
        raw_wheels = []
    if not isinstance(raw_records, list):
        failures.append(
            _failure(
                "build-host native record list is invalid",
                expected="list of safe basenames",
                actual=type(raw_records).__name__,
                repair="bash scripts/release.sh --candidate",
            )
        )
        raw_records = []
    names: list[str] = []
    for value in [*raw_wheels, *raw_records]:
        safe = _safe_basename(value)
        if safe is None:
            failures.append(
                _failure(
                    "build-host returned unsafe filename",
                    expected="one safe artifact basename",
                    actual=str(value),
                    repair="bash scripts/release.sh --candidate",
                )
            )
        else:
            names.append(safe)
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        failures.append(
            _failure(
                "build-host returned duplicate filenames",
                expected="unique artifact basenames",
                actual=", ".join(duplicates),
                repair="bash scripts/release.sh --candidate",
            )
        )
    wheel_by_role: dict[str, str] = {}
    for value in raw_wheels:
        safe = _safe_basename(value)
        if safe is None:
            continue
        role = _wheel_role(safe)
        if role is None:
            failures.append(
                _failure(
                    "build-host returned unexpected macOS wheel",
                    expected=", ".join(sorted(_expected_macos_wheels_by_role())),
                    actual=safe,
                    repair="bash scripts/release.sh --candidate",
                )
            )
            continue
        if role in wheel_by_role:
            failures.append(
                _failure(
                    "build-host returned duplicate macOS wheel role",
                    expected="one wheel for every declared macOS role",
                    actual=role,
                    repair="bash scripts/release.sh --candidate",
                )
            )
        wheel_by_role[role] = safe
    safe_records = tuple(
        safe for value in raw_records if (safe := _safe_basename(value)) is not None
    )
    expected_wheels_by_role = _expected_macos_wheels_by_role()
    if set(wheel_by_role) != set(expected_wheels_by_role):
        failures.append(
            _failure(
                "build-host returned wrong macOS wheel set",
                expected=", ".join(sorted(expected_wheels_by_role)),
                actual=", ".join(sorted(wheel_by_role)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    expected_records = set(_expected_native_record_names())
    if set(safe_records) != expected_records:
        failures.append(
            _failure(
                "build-host returned wrong native record set",
                expected=", ".join(sorted(expected_records)),
                actual=", ".join(sorted(safe_records)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    if failures:
        raise BuildHostError(failures)
    return (
        tuple(wheel_by_role[role] for role in expected_wheels_by_role),
        _expected_native_record_names(),
    )


class ExternalBuildHostChannel:
    """External command adapter; command and credentials live outside source."""

    def __init__(
        self,
        command: Sequence[str],
        *,
        runner: Runner = subprocess.run,
        cohort_id_factory: IdFactory | None = None,
        file_copier: FileCopier = shutil.copyfile,
    ) -> None:
        if not command:
            raise ValueError("build-host command is required")
        self._command = tuple(command)
        self._runner = runner
        self._cohort_id_factory = cohort_id_factory or (lambda: uuid.uuid4().hex)
        self._file_copier = file_copier

    @classmethod
    def from_env(
        cls,
        env: Mapping[str, str],
        *,
        runner: Runner = subprocess.run,
        cohort_id_factory: IdFactory | None = None,
        file_copier: FileCopier = shutil.copyfile,
    ) -> ExternalBuildHostChannel:
        try:
            command = shlex.split(env.get("RELEASE_BUILD_HOST_CHANNEL", ""))
        except ValueError as exc:
            raise BuildHostError(
                [
                    _failure(
                        "build-host channel configuration is invalid",
                        expected="shell-style command tokens",
                        actual=str(exc),
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            ) from None
        if not command:
            raise BuildHostError(
                [
                    _failure(
                        "build-host channel is not configured",
                        expected="RELEASE_BUILD_HOST_CHANNEL command",
                        actual="<missing>",
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )
        return cls(
            command,
            runner=runner,
            cohort_id_factory=cohort_id_factory,
            file_copier=file_copier,
        )

    def _cleanup_remote(self, *, cohort_id: str, source_bundle: SourceBundle) -> None:
        try:
            result = _run(
                self._runner,
                [*self._command, "cleanup", cohort_id, source_bundle.sha256],
            )
        except BaseException as exc:
            raise BuildHostError(
                [
                    _failure(
                        "build-host remote cleanup failed",
                        expected="external build-host cleanup completed",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            ) from None
        if result.returncode != 0:
            raise BuildHostError(
                [
                    _failure(
                        "build-host remote cleanup failed",
                        expected="external build-host cleanup exit 0",
                        actual=result.stderr.strip()
                        or result.stdout.strip()
                        or f"exit {result.returncode}",
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )

    def _cleanup_local(
        self,
        targets: Sequence[tuple[Path, DirectoryIdentity | None]],
        *,
        failure_label: str,
    ) -> BuildHostError | None:
        failures: list[Failure] = []
        preserved: list[Path] = []
        for path, identity in targets:
            try:
                if any(path in preserved_path.parents for preserved_path in preserved):
                    failures.append(
                        _failure(
                            failure_label,
                            expected="owned build-host transients removed",
                            actual="preserved descendant residue",
                            repair="bash scripts/release.sh --candidate",
                        )
                    )
                    continue
                if identity is not None:
                    identity_failure = _directory_identity_failure(identity)
                    if identity_failure is not None:
                        preserved.append(path)
                        failures.append(
                            _failure(
                                failure_label,
                                expected="owned build-host transients removed",
                                actual=f"{identity.label} residue",
                                repair="bash scripts/release.sh --candidate",
                            )
                        )
                        continue
                if path.is_dir() and not path.is_symlink():
                    shutil.rmtree(path)
                elif path.exists() or path.is_symlink():
                    path.unlink()
            except BaseException as exc:
                preserved.append(path)
                failures.append(
                    _failure(
                        failure_label,
                        expected="owned build-host transients removed",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                )
        return BuildHostError(failures) if failures else None

    def _request_payload(
        self,
        *,
        cohort_id: str,
        source_bundle: SourceBundle,
        expected_commit: str,
    ) -> dict[str, object]:
        macos_wheels = list(_expected_macos_wheel_names())
        native_records = list(_expected_native_record_names())
        payload: dict[str, object] = {
            "schema_version": 1,
            "cohort_id": cohort_id,
            "expected_commit": expected_commit,
            "expected_version": _expected_release_version(),
            "source_bundle": {
                "path": "source.bundle",
                "source_commit": source_bundle.source_commit,
                "sha256": source_bundle.sha256,
                "bytes": source_bundle.bytes,
            },
            "expected_outputs": {
                "macos_wheels": macos_wheels,
                "native_records": native_records,
            },
            "paths": {
                "response": "response.json",
                "output_dir": "output",
            },
        }
        if set(payload) != REQUEST_KEYS:
            raise AssertionError("build-host request key set drifted")
        source_payload = payload["source_bundle"]
        output_payload = payload["expected_outputs"]
        paths_payload = payload["paths"]
        if not isinstance(source_payload, Mapping) or set(source_payload) != (
            REQUEST_BUNDLE_KEYS
        ):
            raise AssertionError("build-host request source bundle key set drifted")
        if not isinstance(output_payload, Mapping) or set(output_payload) != (
            REQUEST_OUTPUT_KEYS
        ):
            raise AssertionError("build-host request output key set drifted")
        if not isinstance(paths_payload, Mapping) or set(paths_payload) != (
            REQUEST_PATH_KEYS
        ):
            raise AssertionError("build-host request path key set drifted")
        return payload

    def _read_response(self, response_path: Path) -> Mapping[str, object]:
        try:
            _validate_regular_file(response_path, label="response")
            return _json_object(response_path.read_text(encoding="utf-8"))
        except OSError as exc:
            raise BuildHostError(
                [
                    _failure(
                        "build-host response could not be read",
                        expected="response.json written by build-host adapter",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            ) from None

    def build_macos(
        self,
        *,
        source_bundle: SourceBundle,
        expected_commit: str,
        output_dir: Path,
    ) -> BuildHostResult:
        if not isinstance(source_bundle, SourceBundle):
            raise BuildHostError(
                [
                    _failure(
                        "build-host source bundle is not verified",
                        expected="SourceBundle from create_source_bundle",
                        actual=type(source_bundle).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )
        cohort_id = self._cohort_id_factory()
        _validate_cohort_id(cohort_id)
        primary_error: BuildHostError | None = None
        build_result: BuildHostResult | None = None
        request_dir = output_dir.parent / f".{output_dir.name}.request-{cohort_id}"
        request_bundle = request_dir / "source.bundle"
        request_path = request_dir / "request.json"
        response_path = request_dir / "response.json"
        request_output_dir = request_dir / "output"
        output_dir_created = False
        output_identity: DirectoryIdentity | None = None
        request_identity: DirectoryIdentity | None = None
        request_output_identity: DirectoryIdentity | None = None
        remote_started = False
        try:
            _validate_source_bundle(
                source_bundle,
                requested_commit=expected_commit,
                label="source bundle",
            )
            _validate_fresh_directory_path(output_dir, label="output")
            _validate_fresh_directory_path(request_dir, label="request")
            output_dir.mkdir(parents=True)
            output_dir_created = True
            output_identity = _capture_directory_identity(output_dir, label="output")
            request_output_dir.mkdir(parents=True)
            request_output_identity = _capture_directory_identity(
                request_output_dir, label="request output"
            )
            request_identity = _capture_directory_identity(request_dir, label="request")
            _validate_source_bundle(
                source_bundle,
                requested_commit=expected_commit,
                label="source bundle before copy",
            )
            try:
                self._file_copier(source_bundle.path, request_bundle)
            except OSError as exc:
                raise BuildHostError(
                    [
                        _failure(
                            "build-host source bundle bytes unavailable",
                            expected="verified bundle copied into request directory",
                            actual=type(exc).__name__,
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                ) from None
            request_source_bundle = SourceBundle(
                path=request_bundle,
                source_commit=source_bundle.source_commit,
                sha256=source_bundle.sha256,
                bytes=source_bundle.bytes,
            )
            _validate_source_bundle(
                request_source_bundle,
                requested_commit=expected_commit,
                label="request source bundle after copy",
            )
            request_payload = self._request_payload(
                cohort_id=cohort_id,
                source_bundle=source_bundle,
                expected_commit=expected_commit,
            )
            request_path.write_bytes(canonical_json_bytes(request_payload))
            _validate_source_bundle(
                source_bundle,
                requested_commit=expected_commit,
                label="source bundle before adapter",
            )
            _validate_source_bundle(
                request_source_bundle,
                requested_commit=expected_commit,
                label="request source bundle before adapter",
            )
            remote_started = True
            result = _run(
                self._runner,
                [*self._command, "build-macos", "request.json"],
                cwd=request_dir,
            )
            if result.returncode != 0:
                raise BuildHostError(
                    [
                        _failure(
                            "build-host macOS build failed",
                            expected="external build-host command exit 0",
                            actual=result.stderr.strip()
                            or result.stdout.strip()
                            or f"exit {result.returncode}",
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                )
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            _validate_source_bundle(
                source_bundle,
                requested_commit=expected_commit,
                label="source bundle after adapter",
            )
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            _validate_source_bundle(
                request_source_bundle,
                requested_commit=expected_commit,
                label="request source bundle after adapter",
            )
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            payload = self._read_response(response_path)
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            key_failures = _key_set_failures(
                "build-host response",
                payload,
                RESPONSE_KEYS,
            )
            if key_failures:
                raise BuildHostError(key_failures)
            if payload.get("schema_version") != 1:
                raise BuildHostError(
                    [
                        _failure(
                            "build-host response schema version is wrong",
                            expected="1",
                            actual=repr(payload.get("schema_version")),
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                )
            if payload.get("cohort_id") != cohort_id:
                raise BuildHostError(
                    [
                        _failure(
                            "build-host response cohort id is wrong",
                            expected=cohort_id,
                            actual=repr(payload.get("cohort_id")),
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                )
            _validate_attestation(
                payload,
                expected_commit=expected_commit,
                source_bundle=source_bundle,
            )
            macos_tool_evidence = _validate_macos_tool_evidence(payload)
            wheel_names, record_names = _names_from_payload(payload)
            returned_names = {*wheel_names, *record_names}
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            actual_names = {path.name for path in request_output_dir.iterdir()}
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            if actual_names != returned_names:
                raise BuildHostError(
                    [
                        _failure(
                            "build-host output directory does not match response",
                            expected=", ".join(sorted(returned_names)),
                            actual=", ".join(sorted(actual_names)) or "<empty>",
                            repair="bash scripts/release.sh --candidate",
                        )
                    ]
                )
            request_paths = tuple(
                request_output_dir / name for name in (*wheel_names, *record_names)
            )
            failures: list[Failure] = []
            for path in request_paths:
                _validate_directory_identities(
                    (request_identity, request_output_identity, output_identity)
                )
                regular_failure = _regular_file_failure("retrieved artifact", path)
                _validate_directory_identities(
                    (request_identity, request_output_identity, output_identity)
                )
                if regular_failure is not None:
                    failures.append(regular_failure)
            if failures:
                raise BuildHostError(failures)
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            for name in (*wheel_names, *record_names):
                os_replace_source = request_output_dir / name
                os_replace_dest = output_dir / name
                _validate_directory_identities(
                    (request_identity, request_output_identity, output_identity)
                )
                try:
                    os_replace_source.rename(os_replace_dest)
                except OSError as exc:
                    raise BuildHostError(
                        [
                            _failure(
                                "build-host retrieved artifact could not be installed",
                                expected="retrieved artifact moved into output directory",
                                actual=type(exc).__name__,
                                repair="bash scripts/release.sh --candidate",
                            )
                        ]
                    ) from None
                _validate_directory_identities(
                    (request_identity, request_output_identity, output_identity)
                )
            _validate_directory_identities(
                (request_identity, request_output_identity, output_identity)
            )
            build_result = BuildHostResult(
                macos_wheels=tuple(output_dir / name for name in wheel_names),
                native_records=tuple(output_dir / name for name in record_names),
                tool_evidence=macos_tool_evidence,
            )
        except BuildHostError as exc:
            primary_error = exc
        except OSError as exc:
            primary_error = BuildHostError(
                [
                    _failure(
                        "build-host filesystem operation failed",
                        expected="request and output directories prepared",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )
        except BaseException as exc:
            primary_error = BuildHostError(
                [
                    _failure(
                        (
                            "build-host channel interrupted"
                            if isinstance(exc, KeyboardInterrupt)
                            else "build-host channel failed"
                        ),
                        expected="build-host adapter completed",
                        actual=type(exc).__name__,
                        repair="bash scripts/release.sh --candidate",
                    )
                ]
            )
        cleanup_error: BuildHostError | None = None
        if remote_started:
            try:
                self._cleanup_remote(cohort_id=cohort_id, source_bundle=source_bundle)
            except BuildHostError as exc:
                cleanup_error = exc
        local_cleanup_targets: list[tuple[Path, DirectoryIdentity | None]] = [
            (request_output_dir, request_output_identity),
            (request_dir, request_identity),
        ]
        if primary_error is not None and output_dir_created:
            local_cleanup_targets.append((output_dir, output_identity))
        local_cleanup = self._cleanup_local(
            local_cleanup_targets,
            failure_label="build-host local cleanup failed",
        )
        cleanup_error = _combine_failures(cleanup_error, local_cleanup)
        combined = _combine_failures(primary_error, cleanup_error)
        if combined is not None:
            raise combined
        if build_result is None:
            raise AssertionError("build-host result was not produced")
        return build_result
