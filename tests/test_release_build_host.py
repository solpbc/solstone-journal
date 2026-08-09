# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from collections.abc import Callable, Sequence
from pathlib import Path

import pytest

import scripts.check_rust_release_manifest as checker
import scripts.release_build_host as build_host
import scripts.release_tool_pins as pins
from scripts.release_build_host import SourceBundle

SOURCE_COMMIT = "a" * 40


def _expected_macos_wheels() -> tuple[str, str, str]:
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


ROOT_WHEEL, CORE_WHEEL, SPEAKERS_ANALYZE_WHEEL = _expected_macos_wheels()


def _run_git(repo: Path, argv: Sequence[str]) -> str:
    result = subprocess.run(
        ["git", *argv],
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def _git_repo(tmp_path: Path) -> tuple[Path, str]:
    repo = tmp_path / "repo"
    repo.mkdir()
    _run_git(repo, ["init"])
    _run_git(repo, ["config", "user.name", "Release Test"])
    _run_git(repo, ["config", "user.email", "release-test"])
    (repo / "tracked.txt").write_text("tracked\n", encoding="utf-8")
    _run_git(repo, ["add", "tracked.txt"])
    _run_git(repo, ["commit", "-m", "fixture"])
    return repo, _run_git(repo, ["rev-parse", "HEAD"])


@pytest.mark.release
def test_source_bundle_uses_head_and_materializes_from_real_git(
    tmp_path: Path,
) -> None:
    repo, commit = _git_repo(tmp_path)
    raw_sha_bundle = tmp_path / "raw-sha.bundle"

    raw = subprocess.run(
        ["git", "bundle", "create", str(raw_sha_bundle), commit],
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    assert raw.returncode != 0

    bundle_path = tmp_path / "source.bundle"
    bundle = build_host.create_source_bundle(
        repo,
        expected_commit=commit,
        output_path=bundle_path,
    )
    assert bundle.path == bundle_path
    assert bundle.source_commit == commit
    assert bundle.bytes == bundle_path.stat().st_size
    assert bundle.sha256 == hashlib.sha256(bundle_path.read_bytes()).hexdigest()
    assert _run_git(repo, ["bundle", "list-heads", str(bundle_path)]) == (
        f"{commit} HEAD"
    )
    _run_git(repo, ["bundle", "verify", str(bundle_path)])

    materialized = tmp_path / "materialized"
    clone = subprocess.run(
        ["git", "clone", str(bundle_path), str(materialized)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert clone.returncode == 0, clone.stderr
    assert _run_git(materialized, ["rev-parse", "HEAD"]) == commit
    assert (materialized / "tracked.txt").read_text(encoding="utf-8") == "tracked\n"


def test_source_bundle_rejects_list_heads_mismatch(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    output_path = tmp_path / "source.bundle"

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        if argv[:3] == ["git", "bundle", "create"]:
            output_path.write_bytes(b"bundle")
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:3] == ["git", "bundle", "list-heads"]:
            return subprocess.CompletedProcess(argv, 0, f"{'b' * 40} HEAD\n", "")
        if argv[:3] == ["git", "bundle", "verify"]:
            return subprocess.CompletedProcess(argv, 0, "ok\n", "")
        raise AssertionError(argv)

    with pytest.raises(build_host.BuildHostError) as exc:
        build_host.create_source_bundle(
            repo,
            expected_commit=SOURCE_COMMIT,
            output_path=output_path,
            runner=runner,
        )

    assert exc.value.failures[0].error == "build-host source bundle HEAD is wrong"


def test_source_bundle_rejects_verify_failure(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    output_path = tmp_path / "source.bundle"

    def runner(
        argv: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        if argv[:3] == ["git", "bundle", "create"]:
            output_path.write_bytes(b"bundle")
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:3] == ["git", "bundle", "list-heads"]:
            return subprocess.CompletedProcess(argv, 0, f"{SOURCE_COMMIT} HEAD\n", "")
        if argv[:3] == ["git", "bundle", "verify"]:
            return subprocess.CompletedProcess(argv, 1, "", "bad bundle")
        raise AssertionError(argv)

    with pytest.raises(build_host.BuildHostError) as exc:
        build_host.create_source_bundle(
            repo,
            expected_commit=SOURCE_COMMIT,
            output_path=output_path,
            runner=runner,
        )

    assert exc.value.failures[0].error == "build-host source bundle verification failed"


def _source_bundle(tmp_path: Path) -> SourceBundle:
    path = tmp_path / "source.bundle"
    path.write_bytes(b"bundle")
    return SourceBundle(
        path=path,
        source_commit=SOURCE_COMMIT,
        sha256=hashlib.sha256(b"bundle").hexdigest(),
        bytes=len(b"bundle"),
    )


ResponseMutator = Callable[[dict[str, object]], object]


def _write_expected(output_dir: Path, names: Sequence[str]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    for name in names:
        (output_dir / name).write_bytes(b"x")


def _inventory(root: Path) -> tuple[tuple[str, bytes], ...]:
    return tuple(
        sorted(
            (
                path.relative_to(root).as_posix(),
                path.read_bytes(),
            )
            for path in root.rglob("*")
            if path.is_file() and not path.is_symlink()
        )
    )


def _outside_target(tmp_path: Path) -> tuple[Path, tuple[tuple[str, bytes], ...]]:
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "sentinel.txt").write_bytes(b"keep")
    (outside / ROOT_WHEEL).write_bytes(b"outside-root")
    return outside, _inventory(outside)


def _channel(
    *,
    tmp_path: Path,
    bundle: SourceBundle,
    mutate_response: ResponseMutator | None = None,
    during_build: Callable[[Path], None] | None = None,
    after_response: Callable[[Path], None] | None = None,
    response_kind: str = "regular",
    output_mutator: Callable[[Path], None] | None = None,
    write_files: bool = True,
    cleanup_code: int = 0,
    build_exception: BaseException | None = None,
    cohort_id: str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    file_copier: build_host.FileCopier = shutil.copyfile,
) -> tuple[build_host.ExternalBuildHostChannel, list[tuple[str, ...]]]:
    calls: list[tuple[str, ...]] = []

    def runner(
        argv: Sequence[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        calls.append(tuple(argv))
        if argv == ["adapter", "quoted arg", "build-macos", "request.json"]:
            if build_exception is not None:
                raise build_exception
            cwd = kwargs.get("cwd")
            assert isinstance(cwd, Path)
            request_path = cwd / "request.json"
            request_text = request_path.read_text(encoding="utf-8")
            assert str(tmp_path) not in request_text
            request = json.loads(request_text)
            assert request["source_bundle"]["path"] == "source.bundle"
            assert request["paths"] == {
                "response": "response.json",
                "output_dir": "output",
            }
            assert (cwd / "source.bundle").read_bytes() == b"bundle"
            assert request["source_bundle"] == {
                "path": "source.bundle",
                "source_commit": bundle.source_commit,
                "sha256": bundle.sha256,
                "bytes": bundle.bytes,
            }
            assert request["expected_outputs"] == {
                "macos_wheels": list(build_host._expected_macos_wheel_names()),
                "native_records": list(build_host._expected_native_record_names()),
            }
            if during_build is not None:
                during_build(cwd)
            wheel_names = list(build_host._expected_macos_wheel_names())
            record_names = list(build_host._expected_native_record_names())
            if write_files:
                _write_expected(
                    cwd / "output",
                    [*wheel_names, *record_names],
                )
            if output_mutator is not None:
                output_mutator(cwd / "output")
            payload: object = {
                "schema_version": 1,
                "cohort_id": request["cohort_id"],
                "attestation": {
                    "source_commit": SOURCE_COMMIT,
                    "clean_tree": True,
                    "bundle_sha256": bundle.sha256,
                    "bundle_bytes": bundle.bytes,
                },
                "tool_evidence": pins.fixture_presign_lane_tool_evidence("macos-arm64"),
                "macos_wheels": wheel_names,
                "native_records": record_names,
            }
            if mutate_response is not None:
                assert isinstance(payload, dict)
                payload = mutate_response(payload)
            response_path = cwd / "response.json"
            if response_kind == "regular":
                response_path.write_text(json.dumps(payload), encoding="utf-8")
            elif response_kind == "symlink":
                target = cwd / "response-target.json"
                target.write_text(json.dumps(payload), encoding="utf-8")
                response_path.symlink_to(target)
            elif response_kind == "directory":
                response_path.mkdir()
            elif response_kind == "invalid-utf8":
                response_path.write_bytes(b"\xff")
            else:
                raise AssertionError(response_kind)
            if after_response is not None:
                after_response(cwd)
            return subprocess.CompletedProcess(argv, 0, "", "")
        if len(argv) == 5 and argv[:3] == ["adapter", "quoted arg", "cleanup"]:
            assert argv[4] == bundle.sha256
            return subprocess.CompletedProcess(argv, cleanup_code, "", "cleanup failed")
        raise AssertionError(argv)

    return (
        build_host.ExternalBuildHostChannel.from_env(
            {"RELEASE_BUILD_HOST_CHANNEL": 'adapter "quoted arg"'},
            runner=runner,
            cohort_id_factory=lambda: cohort_id,
            file_copier=file_copier,
        ),
        calls,
    )


def test_external_channel_validates_attestation_and_uses_shlex(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)
    output_dir = tmp_path / "out"
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle)

    result = channel.build_macos(
        source_bundle=bundle,
        expected_commit=SOURCE_COMMIT,
        output_dir=output_dir,
    )

    assert [path.name for path in result.macos_wheels] == [
        *build_host._expected_macos_wheel_names(),
    ]
    assert [path.name for path in result.native_records] == [
        *build_host._expected_native_record_names(),
    ]
    assert calls == [
        ("adapter", "quoted arg", "build-macos", "request.json"),
        (
            "adapter",
            "quoted arg",
            "cleanup",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            bundle.sha256,
        ),
    ]
    assert not (tmp_path / ".out.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").exists()


def test_external_channel_rejects_unverified_path_bundle_input(tmp_path: Path) -> None:
    bundle = _source_bundle(tmp_path)
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(  # type: ignore[arg-type]
            source_bundle=bundle.path,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures[0].error == "build-host source bundle is not verified"


def test_external_channel_rejects_metadata_only_source_bundle_and_cleans(
    tmp_path: Path,
) -> None:
    missing_bundle = SourceBundle(
        path=tmp_path / "missing.bundle",
        source_commit=SOURCE_COMMIT,
        sha256="0" * 64,
        bytes=123,
    )
    channel, calls = _channel(tmp_path=tmp_path, bundle=missing_bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=missing_bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    errors = [failure.error for failure in exc.value.failures]
    assert "build-host source bundle is not a regular file" in errors
    assert calls == []
    assert not (tmp_path / "out").exists()
    assert not (tmp_path / ".out.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").exists()


def test_external_channel_rejects_source_commit_mismatch(tmp_path: Path) -> None:
    bundle = _source_bundle(tmp_path)
    bad_bundle = SourceBundle(
        path=bundle.path,
        source_commit="b" * 40,
        sha256=bundle.sha256,
        bytes=bundle.bytes,
    )
    channel, calls = _channel(tmp_path=tmp_path, bundle=bad_bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bad_bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert (
        exc.value.failures[0].error == "build-host source bundle source commit is wrong"
    )
    assert calls == []


def test_external_channel_rejects_bundle_mutation_between_construction_and_use(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)
    bundle.path.write_bytes(b"tampered")
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any("SHA-256 changed" in failure.error for failure in exc.value.failures)
    assert calls == []


def test_external_channel_rejects_bundle_mutation_during_copy(tmp_path: Path) -> None:
    bundle = _source_bundle(tmp_path)

    def tampering_copy(_src: Path, dst: Path) -> None:
        dst.write_bytes(b"tampered")

    channel, calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        file_copier=tampering_copy,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any(
        "after copy SHA-256 changed" in failure.error for failure in exc.value.failures
    )
    assert calls == []
    assert not (tmp_path / "out").exists()


@pytest.mark.parametrize("which", ["original", "request"])
def test_external_channel_rejects_bundle_mutation_around_adapter_invocation(
    tmp_path: Path, which: str
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate(cwd: Path) -> None:
        if which == "original":
            bundle.path.write_bytes(b"tampered")
        else:
            (cwd / "source.bundle").write_bytes(b"tampered")

    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle, during_build=mutate)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any(
        "after adapter SHA-256 changed" in failure.error
        for failure in exc.value.failures
    )
    assert calls[-1] == (
        "adapter",
        "quoted arg",
        "cleanup",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        bundle.sha256,
    )


@pytest.mark.parametrize("kind", ["symlink", "directory"])
def test_external_channel_rejects_non_regular_bundle_path(
    tmp_path: Path, kind: str
) -> None:
    bundle = _source_bundle(tmp_path)
    target = tmp_path / "bundle-target"
    if kind == "symlink":
        target.write_bytes(b"bundle")
        bundle.path.unlink()
        bundle.path.symlink_to(target)
    else:
        bundle.path.unlink()
        bundle.path.mkdir()
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert (
        exc.value.failures[0].error == "build-host source bundle is not a regular file"
    )
    assert calls == []


@pytest.mark.parametrize(
    "cohort_id",
    [
        "a" * 31,
        "A" * 32,
        "g" * 32,
    ],
)
def test_external_channel_rejects_invalid_cohort_ids_without_channel_action(
    tmp_path: Path, cohort_id: str
) -> None:
    bundle = _source_bundle(tmp_path)
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle, cohort_id=cohort_id)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures[0].error == "build-host cohort id is invalid"
    assert calls == []


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("source_commit", "b" * 40),
        ("clean_tree", False),
        ("bundle_sha256", "0" * 64),
        ("bundle_bytes", 999),
    ],
)
def test_external_channel_rejects_attestation_mismatch(
    tmp_path: Path, key: str, value: object
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate(payload: dict[str, object]) -> dict[str, object]:
        attestation = payload["attestation"]
        assert isinstance(attestation, dict)
        attestation[key] = value
        return payload

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any("attestation" in failure.error for failure in exc.value.failures)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda payload: {**payload, "extra": "ambiguous"},
        lambda payload: {
            key: value for key, value in payload.items() if key != "native_records"
        },
        lambda payload: {
            key: value for key, value in payload.items() if key != "tool_evidence"
        },
    ],
)
def test_external_channel_rejects_response_top_level_key_drift(
    tmp_path: Path, mutate: ResponseMutator
) -> None:
    bundle = _source_bundle(tmp_path)
    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures[0].error == "build-host response key set is invalid"


@pytest.mark.parametrize(
    "mutate",
    [
        lambda attestation: {**attestation, "extra": "ambiguous"},
        lambda attestation: {
            key: value for key, value in attestation.items() if key != "bundle_sha256"
        },
    ],
)
def test_external_channel_rejects_response_attestation_key_drift(
    tmp_path: Path, mutate: Callable[[dict[str, object]], dict[str, object]]
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate_response(payload: dict[str, object]) -> dict[str, object]:
        attestation = payload["attestation"]
        assert isinstance(attestation, dict)
        payload["attestation"] = mutate(attestation)
        return payload

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate_response,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert (
        exc.value.failures[0].error
        == "build-host response attestation key set is invalid"
    )


@pytest.mark.parametrize(
    "mutate_tools",
    [
        lambda tools: {key: value for key, value in tools.items() if key != "swift"},
        lambda tools: {**tools, "swift": "Apple Swift 6.3.3"},
        lambda tools: {**tools, "team": "7QCG8V4M6H"},
    ],
)
def test_external_channel_rejects_forged_macos_tool_evidence(
    tmp_path: Path,
    mutate_tools: Callable[[dict[str, str]], dict[str, str]],
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate_response(payload: dict[str, object]) -> dict[str, object]:
        tools = payload["tool_evidence"]
        assert isinstance(tools, dict)
        payload["tool_evidence"] = mutate_tools(tools)
        return payload

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate_response,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures


def test_external_channel_rejects_non_mapping_response(tmp_path: Path) -> None:
    bundle = _source_bundle(tmp_path)
    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=lambda _payload: [],
        write_files=False,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures[0].error == "build-host response is not an object"


@pytest.mark.parametrize(
    ("response_kind", "expected_error"),
    [
        ("symlink", "build-host response is not a regular file"),
        ("directory", "build-host response is not a regular file"),
        ("invalid-utf8", "build-host channel failed"),
    ],
)
def test_external_channel_rejects_hostile_response_file(
    tmp_path: Path, response_kind: str, expected_error: str
) -> None:
    bundle = _source_bundle(tmp_path)
    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        response_kind=response_kind,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert exc.value.failures[0].error == expected_error


@pytest.mark.parametrize(
    "bad_name",
    [
        "/tmp/root.whl",
        "dir/root.whl",
        "dir\\root.whl",
        ".",
        "..",
    ],
)
def test_external_channel_rejects_unsafe_filename(
    tmp_path: Path, bad_name: str
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate(payload: dict[str, object]) -> dict[str, object]:
        payload["macos_wheels"] = [bad_name, CORE_WHEEL, SPEAKERS_ANALYZE_WHEEL]
        return payload

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any(
        failure.error == "build-host returned unsafe filename"
        for failure in exc.value.failures
    )


def test_external_channel_rejects_one_byte_filename_skew_before_acceptance(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)
    skewed_name = f"{ROOT_WHEEL[:-5]}x.whl"

    def mutate(payload: dict[str, object]) -> dict[str, object]:
        payload["macos_wheels"] = [skewed_name, CORE_WHEEL, SPEAKERS_ANALYZE_WHEEL]
        return payload

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=mutate,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any(
        failure.error == "build-host returned unexpected macOS wheel"
        for failure in exc.value.failures
    )
    assert not (tmp_path / "out").exists()


@pytest.mark.parametrize("kind", ["symlink", "directory"])
def test_external_channel_rejects_hostile_output_entry(
    tmp_path: Path, kind: str
) -> None:
    bundle = _source_bundle(tmp_path)

    def mutate_output(output_dir: Path) -> None:
        entry = output_dir / ROOT_WHEEL
        entry.unlink()
        if kind == "symlink":
            entry.symlink_to("target.whl")
        else:
            entry.mkdir()

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        output_mutator=mutate_output,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert (
        exc.value.failures[0].error
        == "build-host retrieved artifact is not a regular file"
    )


@pytest.mark.parametrize("which", ["request", "output"])
def test_external_channel_rejects_parent_symlink_replacement_without_touching_target(
    tmp_path: Path, which: str
) -> None:
    bundle = _source_bundle(tmp_path)
    outside, before = _outside_target(tmp_path)
    output_dir = tmp_path / "out"

    def replace_parent(cwd: Path) -> None:
        target = cwd if which == "request" else output_dir
        shutil.rmtree(target)
        target.symlink_to(outside, target_is_directory=True)

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        after_response=replace_parent,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=output_dir,
        )

    assert (
        exc.value.failures[0].error == f"build-host {which} directory identity changed"
    )
    assert _inventory(outside) == before


def test_external_channel_rejects_request_output_symlink_replacement_without_touching_target(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)
    outside, before = _outside_target(tmp_path)

    def replace_request_output(cwd: Path) -> None:
        shutil.rmtree(cwd / "output")
        (cwd / "output").symlink_to(outside, target_is_directory=True)

    channel, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        after_response=replace_request_output,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert (
        exc.value.failures[0].error
        == "build-host request output directory identity changed"
    )
    assert _inventory(outside) == before


def test_external_channel_rechecks_parent_after_first_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = _source_bundle(tmp_path)
    outside, before = _outside_target(tmp_path)
    request_dir = tmp_path / ".out.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    original = build_host._directory_identity_failure
    state = {"replaced": False}

    def replace_after_first_check(
        identity: build_host.DirectoryIdentity,
    ) -> build_host.Failure | None:
        failure = original(identity)
        if failure is None and identity.label == "output" and not state["replaced"]:
            state["replaced"] = True
            shutil.rmtree(request_dir)
            request_dir.symlink_to(outside, target_is_directory=True)
        return failure

    monkeypatch.setattr(
        build_host,
        "_directory_identity_failure",
        replace_after_first_check,
    )
    channel, _calls = _channel(tmp_path=tmp_path, bundle=bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    assert any(
        failure.error == "build-host request directory identity changed"
        for failure in exc.value.failures
    )
    assert _inventory(outside) == before


def test_external_channel_rejects_duplicates_extras_and_stale_output(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)

    def duplicate_payload(payload: dict[str, object]) -> dict[str, object]:
        payload["macos_wheels"] = [ROOT_WHEEL, ROOT_WHEEL, SPEAKERS_ANALYZE_WHEEL]
        return payload

    duplicate, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=duplicate_payload,
    )
    with pytest.raises(build_host.BuildHostError):
        duplicate.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "dup",
        )

    def extra_payload(payload: dict[str, object]) -> dict[str, object]:
        payload["macos_wheels"] = [
            ROOT_WHEEL,
            CORE_WHEEL,
            SPEAKERS_ANALYZE_WHEEL,
            "extra.whl",
        ]
        return payload

    extra, _calls = _channel(
        tmp_path=tmp_path, bundle=bundle, mutate_response=extra_payload
    )
    with pytest.raises(build_host.BuildHostError):
        extra.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "extra",
        )

    stale_output = tmp_path / "stale"
    stale_output.mkdir()
    (stale_output / "old.whl").write_bytes(b"old")
    ok, calls = _channel(tmp_path=tmp_path, bundle=bundle)
    with pytest.raises(build_host.BuildHostError) as exc:
        ok.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=stale_output,
        )

    assert exc.value.failures[0].error == "build-host output directory is not empty"
    assert calls == []


@pytest.mark.parametrize(
    ("which", "kind", "expected_error"),
    [
        ("output", "symlink", "build-host output directory is unsafe"),
        ("output", "file", "build-host output directory is unsafe"),
        ("request", "symlink", "build-host request directory is unsafe"),
        ("request", "file", "build-host request directory is unsafe"),
        ("request", "stale", "build-host request directory is not empty"),
    ],
)
def test_external_channel_rejects_unsafe_request_and_output_dirs(
    tmp_path: Path, which: str, kind: str, expected_error: str
) -> None:
    bundle = _source_bundle(tmp_path)
    output_dir = tmp_path / "out"
    target = (
        output_dir
        if which == "output"
        else tmp_path / ".out.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    )
    if kind == "symlink":
        target.symlink_to(tmp_path, target_is_directory=True)
    elif kind == "file":
        target.write_text("not a dir", encoding="utf-8")
    elif kind == "stale":
        target.mkdir()
        (target / "old").write_text("old", encoding="utf-8")
    channel, calls = _channel(tmp_path=tmp_path, bundle=bundle)

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=output_dir,
        )

    assert exc.value.failures[0].error == expected_error
    assert calls == []


def test_external_channel_cleanup_failure_surfaces_on_success_and_failure(
    tmp_path: Path,
) -> None:
    bundle = _source_bundle(tmp_path)
    success, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        cleanup_code=1,
    )
    with pytest.raises(build_host.BuildHostError) as exc:
        success.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "success",
        )
    assert exc.value.failures[0].error == "build-host remote cleanup failed"
    assert not (tmp_path / ".success.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").exists()

    primary, _calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        mutate_response=lambda _payload: [],
        write_files=False,
        cleanup_code=1,
    )
    with pytest.raises(build_host.BuildHostError) as exc:
        primary.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "primary",
        )

    errors = [failure.error for failure in exc.value.failures]
    assert "build-host response is not an object" in errors
    assert "build-host remote cleanup failed" in errors
    assert not (tmp_path / "primary").exists()
    assert not (tmp_path / ".primary.request-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").exists()


@pytest.mark.parametrize(
    ("build_exception", "expected_error"),
    [
        (OSError("spawn failed"), "build-host filesystem operation failed"),
        (KeyboardInterrupt(), "build-host channel interrupted"),
        (SystemExit(2), "build-host channel failed"),
        (AssertionError("boom"), "build-host channel failed"),
        (TypeError("wrong type"), "build-host channel failed"),
    ],
)
def test_external_channel_cleanup_aggregates_runner_errors_and_interruption(
    tmp_path: Path, build_exception: BaseException, expected_error: str
) -> None:
    bundle = _source_bundle(tmp_path)
    channel, calls = _channel(
        tmp_path=tmp_path,
        bundle=bundle,
        build_exception=build_exception,
        cleanup_code=1,
    )

    with pytest.raises(build_host.BuildHostError) as exc:
        channel.build_macos(
            source_bundle=bundle,
            expected_commit=SOURCE_COMMIT,
            output_dir=tmp_path / "out",
        )

    errors = [failure.error for failure in exc.value.failures]
    assert expected_error in errors
    assert "build-host remote cleanup failed" in errors
    assert calls[-1] == (
        "adapter",
        "quoted arg",
        "cleanup",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        bundle.sha256,
    )
