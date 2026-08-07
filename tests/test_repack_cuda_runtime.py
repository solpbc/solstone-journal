# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import contextlib
import gzip
import hashlib
import io
import json
import socket
import tarfile
from collections.abc import Iterator
from pathlib import Path
from unittest import mock

import httpx
import pytest

from scripts import repack_cuda_runtime as runtime
from solstone.think.providers.oci_image import OciImageError, _extract_layer

REPO = "ggml-org/llama.cpp"
TAG = "server-cuda13-b10068"
IMAGE_REF = f"ghcr.io/{REPO}:{TAG}"
ARCH = "amd64"
SECOND_ARCH = "arm64"
TIMESTAMP = 1_800_000_000
# Fixed gzip header time keeps mock layer digests stable regardless of suite time.
LAYER_GZIP_MTIME = 0
REVISION = "abcdef1234567890"
EULA_BYTES = b"fixture NVIDIA CUDA EULA\n"
LLAMA_LICENSE_BYTES = b"MIT License\n\nCopyright (c) 2023-2026 The ggml authors\n"


@pytest.fixture(scope="module", autouse=True)
def block_real_network() -> Iterator[None]:
    patch = pytest.MonkeyPatch()

    def blocked_connect(*_args, **_kwargs):
        raise AssertionError("real network disabled in repack CUDA runtime tests")

    patch.setattr(socket.socket, "connect", blocked_connect)
    patch.setattr(socket.socket, "connect_ex", blocked_connect)
    pin = runtime.local_install.CudaServerPin(
        cuda_version=13,
        embedded_arch_set=frozenset({"sm_86", "sm_89", "sm_120a", "sm_121a"}),
        binary_name="llama-server",
        device_flag_value="CUDA0",
        visible_devices_env="CUDA_VISIBLE_DEVICES",
        shared_wanted_files=(
            "llama-server", "libllama-server-impl.so", "libllama-common.so.0",
            "libmtmd.so.0", "libllama.so.0", "libggml.so.0", "libggml-base.so.0",
            "libggml-cuda.so", "libcudart.so.13", "libcublas.so.13", "libcublasLt.so.13",
        ),
        cpu_wanted_files_by_arch={
            "amd64": (
                "libggml-cpu-x64.so", "libggml-cpu-sse42.so", "libggml-cpu-sandybridge.so",
                "libggml-cpu-ivybridge.so", "libggml-cpu-piledriver.so", "libggml-cpu-haswell.so",
                "libggml-cpu-skylakex.so", "libggml-cpu-cannonlake.so", "libggml-cpu-cascadelake.so",
                "libggml-cpu-icelake.so", "libggml-cpu-cooperlake.so", "libggml-cpu-zen4.so",
                "libggml-cpu-alderlake.so", "libggml-cpu-sapphirerapids.so",
            ),
            "arm64": (
                "libggml-cpu-armv8.0_1.so", "libggml-cpu-armv8.2_1.so", "libggml-cpu-armv8.2_2.so",
                "libggml-cpu-armv8.2_3.so", "libggml-cpu-armv8.6_1.so", "libggml-cpu-armv8.6_2.so",
                "libggml-cpu-armv9.2_1.so", "libggml-cpu-armv9.2_2.so",
            ),
        },
        artifacts_by_key={},
    )
    patch.setattr(runtime.local_install, "cuda_server_pin", lambda: pin)
    yield
    patch.undo()


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _digest_ref(data: bytes) -> str:
    return f"sha256:{_sha256(data)}"


def _json_bytes(data: dict) -> bytes:
    return json.dumps(data, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _regular(
    archive: tarfile.TarFile, name: str, data: bytes, mode: int = 0o644
) -> None:
    member = tarfile.TarInfo(name)
    member.size = len(data)
    member.mode = mode
    archive.addfile(member, io.BytesIO(data))


def _symlink(archive: tarfile.TarFile, name: str, target: str) -> None:
    member = tarfile.TarInfo(name)
    member.type = tarfile.SYMTYPE
    member.linkname = target
    archive.addfile(member)


def _layer(entries: list[tuple[str, bytes | str, str]]) -> bytes:
    buffer = io.BytesIO()
    with gzip.GzipFile(
        filename="",
        mode="wb",
        fileobj=buffer,
        compresslevel=9,
        mtime=LAYER_GZIP_MTIME,
    ) as gz:
        with tarfile.open(fileobj=gz, mode="w") as archive:
            for name, data, kind in entries:
                if kind == "symlink":
                    assert isinstance(data, str)
                    _symlink(archive, name, data)
                else:
                    assert isinstance(data, bytes)
                    mode = 0o755 if name.endswith("llama-server") else 0o644
                    _regular(archive, name, data, mode=mode)
    return buffer.getvalue()


@contextlib.contextmanager
def _pinned_clock(now: float) -> Iterator[None]:
    # gzip.time is the global time module; keep this patch scoped to one build.
    with mock.patch.object(gzip.time, "time", lambda: now):
        yield


class _Registry:
    def __init__(
        self,
        manifests: dict[str, bytes],
        blobs: dict[str, bytes],
        *,
        layer_refs: tuple[str, ...],
        corrupt_blob: str | None = None,
    ) -> None:
        self.manifests = manifests
        self.blobs = blobs
        self.layer_refs = layer_refs
        self.corrupt_blob = corrupt_blob
        self.requests: list[httpx.Request] = []

    def client(self) -> httpx.Client:
        return httpx.Client(transport=httpx.MockTransport(self.handle))

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        path = request.url.path
        if path == "/token":
            return httpx.Response(200, json={"token": "token-1"}, request=request)

        manifest_prefix = f"/v2/{REPO}/manifests/"
        if path.startswith(manifest_prefix):
            ref = path.removeprefix(manifest_prefix)
            body = self.manifests.get(ref)
            if body is None:
                return httpx.Response(404, request=request)
            return httpx.Response(
                200,
                content=body,
                headers={"Docker-Content-Digest": _digest_ref(body)},
                request=request,
            )

        blob_prefix = f"/v2/{REPO}/blobs/"
        if path.startswith(blob_prefix):
            digest = path.removeprefix(blob_prefix)
            body = self.blobs.get(digest)
            if body is None:
                return httpx.Response(404, request=request)
            if digest == self.corrupt_blob:
                body = b"corrupt"
            return httpx.Response(200, content=body, request=request)

        return httpx.Response(404, request=request)


def _wanted_files() -> tuple[str, ...]:
    return runtime.local_install.cuda_server_pin().wanted_files_for_arch(ARCH)


def _base_entries(
    *,
    omit: str | None = None,
    ambiguous: bool = False,
    unexpected_cpu: bool = False,
    unexpected_sibling: bool = True,
    eula_bytes: bytes = EULA_BYTES,
    second_eula_bytes: bytes | None = None,
) -> tuple[list[tuple[str, bytes | str, str]], list[tuple[str, bytes | str, str]]]:
    lower: list[tuple[str, bytes | str, str]] = [
        ("a/libllama.so.0", b"lower decoy", "regular"),
        ("usr/lib/libllama-common.so.0", b"lower common decoy", "regular"),
    ]
    upper: list[tuple[str, bytes | str, str]] = [
        ("a/.wh.libllama.so.0", b"", "regular"),
        ("usr/lib/libllama.so.0", b"upper libllama", "regular"),
        ("usr/lib/libllama-common.so.0", b"upper common", "regular"),
        (
            "usr/local/cuda/lib64/libcudart.so.13.3.29",
            b"resolved libcudart",
            "regular",
        ),
        (
            "usr/local/cuda/lib64/libcudart-middle.so",
            "libcudart.so.13.3.29",
            "symlink",
        ),
        (
            "usr/local/cuda/lib64/libcudart.so.13",
            "libcudart-middle.so",
            "symlink",
        ),
        (
            "var/lib/dpkg/status",
            (
                b"Package: cuda-cudart-13-3\nVersion: 13.3.29-1\n\n"
                b"Package: libcublas-13-3\nVersion: 13.5.1.27-1\n\n"
            ),
            "regular",
        ),
    ]
    for relpath in runtime.NVIDIA_EULA_DOC_PATHS:
        payload = (
            second_eula_bytes
            if second_eula_bytes is not None and "libcublas" in str(relpath)
            else eula_bytes
        )
        upper.append((relpath.as_posix(), payload, "regular"))
    if unexpected_sibling:
        upper.append(("usr/lib/libbonus.so.1", b"bonus", "regular"))
    if unexpected_cpu:
        upper.append(("usr/lib/libggml-cpu-surprise.so", b"surprise", "regular"))
    for name in _wanted_files():
        if name == omit or name in {
            "libcudart.so.13",
            "libllama.so.0",
            "libllama-common.so.0",
        }:
            continue
        upper.append((f"usr/lib/{name}", f"bytes for {name}".encode(), "regular"))
    if ambiguous:
        upper.append(("opt/libggml.so.0", b"different libggml", "regular"))
    return lower, upper


def _registry(
    *,
    omit: str | None = None,
    ambiguous: bool = False,
    missing_revision: bool = False,
    second_arch_missing_revision: bool = False,
    unexpected_cpu: bool = False,
    unexpected_sibling: bool = True,
    eula_bytes: bytes = EULA_BYTES,
    second_eula_bytes: bytes | None = None,
    corrupt_layer: bool = False,
) -> _Registry:
    lower_entries, upper_entries = _base_entries(
        omit=omit,
        ambiguous=ambiguous,
        unexpected_cpu=unexpected_cpu,
        unexpected_sibling=unexpected_sibling,
        eula_bytes=eula_bytes,
        second_eula_bytes=second_eula_bytes,
    )
    layers = [_layer(lower_entries), _layer(upper_entries)]
    layer_refs = [_digest_ref(data) for data in layers]
    config = {
        "config": {
            "Env": [
                "NV_CUDA_CUDART_VERSION=13.3.29-1",
                "NV_LIBCUBLAS_VERSION=13.5.1.27-1",
            ],
            "Labels": {} if missing_revision else {runtime.REVISION_LABEL: REVISION},
        }
    }
    config_bytes = _json_bytes(config)
    config_ref = _digest_ref(config_bytes)
    manifest = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_ref,
            "size": len(config_bytes),
        },
        "layers": [
            {
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": digest,
                "size": len(data),
            }
            for digest, data in zip(layer_refs, layers, strict=True)
        ],
    }
    manifest_bytes = _json_bytes(manifest)
    manifest_ref = _digest_ref(manifest_bytes)
    manifests = [
        {
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": manifest_ref,
            "platform": {"architecture": ARCH, "os": "linux"},
        }
    ]
    manifest_bodies = {manifest_ref: manifest_bytes}
    blobs = {
        config_ref: config_bytes,
        **{digest: data for digest, data in zip(layer_refs, layers, strict=True)},
    }
    if second_arch_missing_revision:
        second_config = {
            "config": {
                "Env": [
                    "NV_CUDA_CUDART_VERSION=13.3.29-1",
                    "NV_LIBCUBLAS_VERSION=13.5.1.27-1",
                ],
                "Labels": {},
            }
        }
        second_config_bytes = _json_bytes(second_config)
        second_config_ref = _digest_ref(second_config_bytes)
        second_manifest = {
            **manifest,
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": second_config_ref,
                "size": len(second_config_bytes),
            },
        }
        second_manifest_bytes = _json_bytes(second_manifest)
        second_manifest_ref = _digest_ref(second_manifest_bytes)
        manifests.append(
            {
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": second_manifest_ref,
                "platform": {"architecture": SECOND_ARCH, "os": "linux"},
            }
        )
        manifest_bodies[second_manifest_ref] = second_manifest_bytes
        blobs[second_config_ref] = second_config_bytes
    index = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": manifests,
    }
    index_bytes = _json_bytes(index)
    return _Registry(
        {
            TAG: index_bytes,
            _digest_ref(index_bytes): index_bytes,
            **manifest_bodies,
        },
        blobs,
        layer_refs=tuple(layer_refs),
        corrupt_blob=layer_refs[0] if corrupt_layer else None,
    )


def _repo_root(tmp_path: Path, eula: bytes = EULA_BYTES) -> Path:
    root = tmp_path / "repo"
    licenses = root / "solstone/licenses"
    licenses.mkdir(parents=True)
    (licenses / "NVIDIA-CUDA-EULA-13.3.txt").write_bytes(eula)
    (licenses / "llama.cpp-LICENSE.txt").write_bytes(LLAMA_LICENSE_BYTES)
    return root


def _run(
    tmp_path: Path,
    registry: _Registry,
    *,
    arches: tuple[str, ...] = (ARCH,),
    timestamp: int = TIMESTAMP,
    expected_eula: bytes = EULA_BYTES,
    repo_eula: bytes = EULA_BYTES,
    monkeypatch: pytest.MonkeyPatch | None = None,
) -> tuple[list[runtime.RepackArtifact], io.StringIO]:
    if monkeypatch is not None:
        monkeypatch.setattr(runtime, "git_commit", lambda _root: ("f" * 40, []))
    stderr = io.StringIO()
    with registry.client() as client:
        artifacts = runtime.repack_cuda_runtime(
            image_ref=IMAGE_REF,
            arches=arches,
            build_timestamp_epoch=timestamp,
            verifier="pytest",
            output_dir=tmp_path / "out",
            client=client,
            expected_eula_sha256=_sha256(expected_eula),
            repo_root=_repo_root(tmp_path, repo_eula),
            stderr=stderr,
        )
    return artifacts, stderr


def _run_one(
    tmp_path: Path,
    registry: _Registry,
    *,
    timestamp: int = TIMESTAMP,
    expected_eula: bytes = EULA_BYTES,
    repo_eula: bytes = EULA_BYTES,
    monkeypatch: pytest.MonkeyPatch | None = None,
) -> tuple[runtime.RepackArtifact, io.StringIO]:
    artifacts, stderr = _run(
        tmp_path,
        registry,
        timestamp=timestamp,
        expected_eula=expected_eula,
        repo_eula=repo_eula,
        monkeypatch=monkeypatch,
    )
    return artifacts[0], stderr


def _assert_no_artifacts(output_dir: Path) -> None:
    assert not list(output_dir.glob("*.tar.gz"))
    assert not list(output_dir.glob("*.sha256"))


def _extract_rootfs(tmp_path: Path, registry: _Registry) -> Path:
    rootfs = tmp_path / "rootfs"
    rootfs.mkdir(parents=True)
    for index, layer_ref in enumerate(registry.layer_refs, start=1):
        layer_path = tmp_path / f"layer-{index}.tar.gz"
        layer_path.write_bytes(registry.blobs[layer_ref])
        _extract_layer(layer_path, rootfs)
    return rootfs


def _read_stage_provenance(artifact: runtime.RepackArtifact) -> dict:
    return json.loads(
        (artifact.stage_dir / "provenance.json").read_text(encoding="utf-8")
    )


def _tar_member_bytes(tarball: Path, name: str) -> bytes:
    with tarfile.open(tarball, "r:gz") as archive:
        member = archive.extractfile(name)
        assert member is not None
        return member.read()


def test_end_to_end_fixture_run_and_drift_policy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact, stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)

    assert artifact.tarball_path.is_file()
    assert artifact.stage_dir.is_dir()
    provenance = _read_stage_provenance(artifact)
    assert provenance["warnings"][0]["code"] == "unexpected_lib_sibling"
    assert "libbonus.so.1" in stderr.getvalue()

    with pytest.raises(OciImageError) as exc_info:
        _run_one(
            tmp_path / "cpu",
            _registry(unexpected_cpu=True),
            monkeypatch=monkeypatch,
        )
    assert exc_info.value.reason_code == "unexpected_cpu_variant"
    assert "libggml-cpu-surprise.so" in str(exc_info.value)

    missing = _wanted_files()[0]
    with pytest.raises(OciImageError) as missing_exc:
        _run_one(
            tmp_path / "missing",
            _registry(omit=missing),
            monkeypatch=monkeypatch,
        )
    assert missing_exc.value.reason_code == "wanted_file_missing"
    assert missing in str(missing_exc.value)


def test_symlink_cell_packages_resolved_bytes_and_records_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact, _stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)
    provenance = _read_stage_provenance(artifact)
    entry = next(
        item for item in provenance["files"] if item["name"] == "libcudart.so.13"
    )

    assert (
        artifact.stage_dir / "libcudart.so.13"
    ).read_bytes() == b"resolved libcudart"
    assert (
        _tar_member_bytes(artifact.tarball_path, "libcudart.so.13")
        == b"resolved libcudart"
    )
    assert entry["resolved_source_path"] == (
        "usr/local/cuda/lib64/libcudart.so.13.3.29"
    )
    assert entry["sha256"] == _sha256(b"resolved libcudart")


def test_unmodified_bytes_generalized(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    registry = _registry()
    artifact, _stderr = _run_one(tmp_path, registry, monkeypatch=monkeypatch)
    provenance = _read_stage_provenance(artifact)
    rootfs = _extract_rootfs(tmp_path / "independent", registry)
    staged_runtime_files = {
        path.name
        for path in artifact.stage_dir.iterdir()
        if path.is_file() and path.name != "provenance.json"
    }

    assert staged_runtime_files == {entry["name"] for entry in provenance["files"]}
    for entry in provenance["files"]:
        staged = artifact.stage_dir / entry["name"]
        source = rootfs / entry["resolved_source_path"]
        assert source.is_file()
        assert _sha256(staged.read_bytes()) == _sha256(source.read_bytes())


def test_whiteout_observability(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact, _stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)

    assert (artifact.stage_dir / "libllama-common.so.0").read_bytes() == b"upper common"
    assert (artifact.stage_dir / "libllama.so.0").read_bytes() == b"upper libllama"
    provenance = _read_stage_provenance(artifact)
    libllama = next(
        item for item in provenance["files"] if item["name"] == "libllama.so.0"
    )
    assert libllama["resolved_source_path"] == "usr/lib/libllama.so.0"


def test_abort_cells(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    with pytest.raises(OciImageError) as corrupt_exc:
        _run_one(
            tmp_path / "corrupt",
            _registry(corrupt_layer=True),
            monkeypatch=monkeypatch,
        )
    assert corrupt_exc.value.reason_code == "sha256_mismatch"
    assert "sha256:" in str(corrupt_exc.value)
    _assert_no_artifacts(tmp_path / "corrupt" / "out")

    with pytest.raises(OciImageError) as revision_exc:
        _run_one(
            tmp_path / "revision",
            _registry(missing_revision=True),
            monkeypatch=monkeypatch,
        )
    assert revision_exc.value.reason_code == "revision_label_missing"
    _assert_no_artifacts(tmp_path / "revision" / "out")

    with pytest.raises(OciImageError) as ambiguous_exc:
        _run_one(
            tmp_path / "ambiguous",
            _registry(ambiguous=True),
            monkeypatch=monkeypatch,
        )
    assert ambiguous_exc.value.reason_code == "wanted_file_ambiguous"
    assert "usr/lib/libggml.so.0" in str(ambiguous_exc.value)
    assert "opt/libggml.so.0" in str(ambiguous_exc.value)
    _assert_no_artifacts(tmp_path / "ambiguous" / "out")

    with pytest.raises(OciImageError) as multi_exc:
        _run(
            tmp_path / "multi",
            _registry(second_arch_missing_revision=True),
            arches=(ARCH, SECOND_ARCH),
            monkeypatch=monkeypatch,
        )
    assert multi_exc.value.reason_code == "revision_label_missing"
    _assert_no_artifacts(tmp_path / "multi" / "out")


def test_eula_gate(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    with pytest.raises(OciImageError) as sha_exc:
        _run_one(
            tmp_path / "sha",
            _registry(eula_bytes=b"changed eula\n"),
            expected_eula=EULA_BYTES,
            monkeypatch=monkeypatch,
        )
    assert sha_exc.value.reason_code == "eula_sha_mismatch"
    assert "trigger CLO re-check" in str(sha_exc.value)
    _assert_no_artifacts(tmp_path / "sha" / "out")

    with pytest.raises(OciImageError) as diff_exc:
        _run_one(
            tmp_path / "diff",
            _registry(second_eula_bytes=b"different eula\n"),
            monkeypatch=monkeypatch,
        )
    assert diff_exc.value.reason_code == "eula_sha_mismatch"
    assert "differ" in str(diff_exc.value)
    _assert_no_artifacts(tmp_path / "diff" / "out")

    with pytest.raises(OciImageError) as repo_exc:
        _run_one(
            tmp_path / "repo",
            _registry(),
            repo_eula=b"wrong committed eula\n",
            monkeypatch=monkeypatch,
        )
    assert repo_exc.value.reason_code == "eula_sha_mismatch"
    _assert_no_artifacts(tmp_path / "repo" / "out")


def test_determinism(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    first, _stderr = _run_one(tmp_path / "one", _registry(), monkeypatch=monkeypatch)
    second, _stderr = _run_one(tmp_path / "two", _registry(), monkeypatch=monkeypatch)
    third, _stderr = _run_one(
        tmp_path / "three",
        _registry(),
        timestamp=TIMESTAMP + 1,
        monkeypatch=monkeypatch,
    )

    assert first.tarball_sha256 == second.tarball_sha256
    assert first.tarball_sha256 != third.tarball_sha256
    first_provenance = _read_stage_provenance(first)
    third_provenance = _read_stage_provenance(third)
    assert first_provenance["build_timestamp_epoch"] == TIMESTAMP
    assert third_provenance["build_timestamp_epoch"] == TIMESTAMP + 1
    first_files = {item["name"]: item["sha256"] for item in first_provenance["files"]}
    third_files = {item["name"]: item["sha256"] for item in third_provenance["files"]}
    assert first_files == third_files


def test_provenance_and_sidecar(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact, _stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)
    provenance = _read_stage_provenance(artifact)

    assert set(provenance) == {
        "arch",
        "build_tag",
        "build_timestamp_epoch",
        "files",
        "index_digest",
        "manifest_digest",
        "notice_file_sha256s",
        "nvidia_packages",
        "pipeline_git_commit",
        "pipeline_script",
        "revision_label",
        "upstream_image_ref",
        "upstream_tag",
        "verifier",
        "version_resolution",
        "warnings",
    }
    assert provenance["verifier"] == "pytest"
    assert provenance["build_timestamp_epoch"] == TIMESTAMP
    assert provenance["build_tag"] == "b10068"
    assert provenance["revision_label"] == REVISION
    assert provenance["version_resolution"] == {
        "cuda-cudart-13-3": "dpkg_status",
        "libcublas-13-3": "dpkg_status",
    }
    for entry in provenance["files"]:
        assert (
            _sha256((artifact.stage_dir / entry["name"]).read_bytes())
            == entry["sha256"]
        )
    sidecar = artifact.sidecar_path.read_text(encoding="utf-8")
    assert sidecar == f"{artifact.tarball_sha256}  {artifact.tarball_path.name}\n"


def test_notices_and_staged_license_files(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact, _stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)

    notice_block = Path(
        "tests/fixtures/repack_cuda_runtime/cuda_notice_block.md"
    ).read_bytes()
    third_party_notices = Path("solstone/THIRD_PARTY_NOTICES.md").read_bytes()
    assert b"\n" + notice_block in third_party_notices
    assert third_party_notices.endswith(notice_block)
    assert (
        hashlib.sha256(
            Path("solstone/licenses/NVIDIA-CUDA-EULA-13.3.txt").read_bytes()
        ).hexdigest()
        == runtime.CUDA_EULA_SHA256
    )
    assert "Copyright (c) 2023-2026 The ggml authors" in Path(
        "solstone/licenses/llama.cpp-LICENSE.txt"
    ).read_text(encoding="utf-8")
    for name in runtime.NOTICE_FILES:
        assert (artifact.stage_dir / "licenses" / name).read_bytes() == (
            tmp_path / "repo" / "solstone/licenses" / name
        ).read_bytes()


def test_pin_snippet_matches_local_install_shape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    artifact, _stderr = _run_one(tmp_path, _registry(), monkeypatch=monkeypatch)

    runtime.print_pin_snippet([artifact])
    payload = json.loads(capsys.readouterr().out)

    assert payload == {
        ARCH: {
            "binary_name": runtime.local_install.cuda_server_pin().binary_name,
            "filename": artifact.tarball_path.name,
            "release_tag": "b10068",
            "sha256": artifact.tarball_sha256,
        }
    }


def test_no_network_fetch_layer_is_injected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    registry = _registry()

    _run_one(tmp_path, registry, monkeypatch=monkeypatch)

    assert registry.requests
    assert all(request.url.host == "ghcr.io" for request in registry.requests)


def test_layer_helper_is_deterministic_across_clock_seconds() -> None:
    _lower_entries, upper_entries = _base_entries()

    with _pinned_clock(1_700_000_000.0):
        first = _layer(upper_entries)
    with _pinned_clock(1_700_000_001.0):
        second = _layer(upper_entries)

    assert first == second
    assert int.from_bytes(first[4:8], "little") == LAYER_GZIP_MTIME
    assert int.from_bytes(second[4:8], "little") == LAYER_GZIP_MTIME

    def members(
        data: bytes,
    ) -> list[tuple[str, bytes, str, int, bytes | None]]:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            records: list[tuple[str, bytes, str, int, bytes | None]] = []
            for member in archive.getmembers():
                payload = None
                if member.isfile():
                    extracted = archive.extractfile(member)
                    assert extracted is not None
                    payload = extracted.read()
                records.append(
                    (member.name, member.type, member.linkname, member.mode, payload)
                )
            return records

    first_members = members(first)
    assert first_members == members(second)
    assert (
        "usr/local/cuda/lib64/libcudart-middle.so",
        tarfile.SYMTYPE,
        "libcudart.so.13.3.29",
        0o644,
        None,
    ) in first_members
    assert (
        "usr/local/cuda/lib64/libcudart.so.13",
        tarfile.SYMTYPE,
        "libcudart-middle.so",
        0o644,
        None,
    ) in first_members


def test_registry_layer_manifests_are_stable_across_clock_seconds() -> None:
    with _pinned_clock(1_700_000_000.0):
        first = _registry()
    with _pinned_clock(1_700_000_001.0):
        second = _registry()

    assert first.layer_refs == second.layer_refs
    assert first.manifests.keys() == second.manifests.keys()
    for key in first.manifests:
        assert first.manifests[key] == second.manifests[key]


def test_full_repack_ignores_mock_layer_build_clock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with _pinned_clock(1_700_000_000.0):
        first_registry = _registry()
    with _pinned_clock(1_700_000_001.0):
        second_registry = _registry()

    first, _stderr = _run_one(tmp_path / "one", first_registry, monkeypatch=monkeypatch)
    second, _stderr = _run_one(
        tmp_path / "two", second_registry, monkeypatch=monkeypatch
    )

    assert first.tarball_sha256 == second.tarball_sha256


def test_former_layer_construction_was_clock_dependent() -> None:
    _lower_entries, upper_entries = _base_entries()

    def former_layer(entries: list[tuple[str, bytes | str, str]]) -> bytes:
        buffer = io.BytesIO()
        # Intentional reproduction of the removed path to prove drift is caught.
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            for name, data, kind in entries:
                if kind == "symlink":
                    assert isinstance(data, str)
                    _symlink(archive, name, data)
                else:
                    assert isinstance(data, bytes)
                    mode = 0o755 if name.endswith("llama-server") else 0o644
                    _regular(archive, name, data, mode=mode)
        return buffer.getvalue()

    with _pinned_clock(1_700_000_000.0):
        first = former_layer(upper_entries)
    with _pinned_clock(1_700_000_001.0):
        second = former_layer(upper_entries)

    assert int.from_bytes(first[4:8], "little") == 1_700_000_000
    assert int.from_bytes(second[4:8], "little") == 1_700_000_001
    assert first != second
    assert _sha256(first) != _sha256(second)
