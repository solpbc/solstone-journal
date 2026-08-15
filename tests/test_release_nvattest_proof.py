# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import hashlib
import json
import os
import platform
import shlex
import shutil
import sys
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

import scripts.release_nvattest_proof as proof
import solstone.think.providers.nvattest_authority as nvattest_authority
from scripts.check_rust_release_manifest import (
    canonical_json_bytes,
    validate_public_evidence_text,
)
from scripts.release_install_smoke import SCRUBBED_COMMAND_ENV
from scripts.release_public_evidence import validate_public_evidence_tree
from scripts.release_target_policy import TARGET_POLICY
from solstone.think.providers import nvattest_install
from solstone.think.providers.nvattest_authority import (
    TARGET_KEYS,
    NvattestTargetKey,
    authority_entry,
    authority_payload,
)
from solstone.think.providers.nvattest_install import SIDECAR_SCHEMA_VERSION
from solstone.think.providers.nvattest_loader import (
    NVATTEST_LIB_RELPATH,
    nvattest_library_env,
)
from tests.helpers.nvattest_fixtures import (
    _write_payload_tarball,
    download_real_archive,
)

SOURCE_COMMIT = "a" * 40
CORE_LOCK = "b" * 64
CANDIDATE_DIGEST = "c" * 64
LEDGER_SHA = "d" * 64
CHALLENGE = "e" * 64
VERSION = "1.0.0"
RECORDED_AT = datetime(2026, 7, 27, 12, 0, tzinfo=UTC)
FIXTURE_DIR = Path(__file__).resolve().parent / "fixtures" / "nvattest"
PROOF_TARGET_BY_KEY = {
    "linux-x86_64": "linux-x86_64-musl",
    "linux-aarch64": "linux-aarch64-musl",
    "macos-arm64": "macos-arm64",
}
SUPPORT_VERSIONS = {
    "anyio": "4.12.1",
    "certifi": "2026.1.4",
    "h11": "0.16.0",
    "httpcore": "1.0.9",
    "httpx": "0.28.1",
    "idna": "3.11",
    "sniffio": "1.3.1",
    "typing-extensions": "4.15.0",
}
PIP_ANCESTOR_STDOUT = """Processing ./wheels/nested/pip_render_probe-0.1.0-py3-none-any.whl
Installing collected packages: pip-render-probe
Successfully installed pip-render-probe-0.1.0
"""
PIP_UNRELATED_STDOUT = """Processing /var/tmp/solstone-checkout-4yw4uak3/a9c8603b-3fbf-4c99-8870-f7c8eb731d22/scratchpad/wheels/nested/pip_render_probe-0.1.0-py3-none-any.whl
Installing collected packages: pip-render-probe
Successfully installed pip-render-probe-0.1.0
"""


@dataclass(frozen=True)
class SyntheticCase:
    target_key: NvattestTargetKey
    target: str
    candidate_dir: Path
    candidate_paths: tuple[Path, ...]
    support_paths: tuple[Path, ...]
    expected_candidate_wheels: list[dict[str, Any]]
    expected_support_distributions: list[dict[str, Any]]
    support_distributions: list[dict[str, Any]]
    archive_path: Path
    manifest_path: Path
    canonical_authority_bytes: bytes
    authority_target: Mapping[str, Any]


def test_production_companion_manifests_match_authority_and_validate() -> None:
    payload = authority_payload()
    for target_key in TARGET_KEYS:
        authority_target = payload["targets"][target_key]
        manifest_identity = authority_target["companion_manifest"]
        path = FIXTURE_DIR / manifest_identity["name"]
        data = path.read_bytes()

        assert hashlib.sha256(data).hexdigest() == manifest_identity["sha256"]
        assert (
            proof.validate_companion_manifest_bytes(
                data,
                target_key=target_key,
                authority_target=authority_target,
            )
            == []
        )


def test_manifest_member_order_differs_from_authority_but_validates() -> None:
    target_key = "linux-x86_64"
    authority_target = authority_payload()["targets"][target_key]
    data = (FIXTURE_DIR / authority_target["companion_manifest"]["name"]).read_bytes()
    manifest = json.loads(data)

    manifest_order = [member["path"] for member in manifest["archive_members"]]
    authority_order = [member["relpath"] for member in authority_target["inventory"]]

    assert manifest_order != authority_order
    assert (
        proof.validate_companion_manifest_bytes(
            data,
            target_key=target_key,
            authority_target=authority_target,
        )
        == []
    )


@pytest.mark.parametrize("target_key", TARGET_KEYS)
def test_synthetic_run_writes_canonical_public_receipt_for_target(
    tmp_path: Path,
    target_key: NvattestTargetKey,
) -> None:
    case = _synthetic_case(tmp_path, target_key)
    output = tmp_path / f"{target_key}.json"
    services = _synthetic_services(case, tmp_path)

    written = proof.run_nvattest_proof(
        target=case.target,
        version=VERSION,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
        candidate_digest=CANDIDATE_DIGEST,
        ledger_sha256=LEDGER_SHA,
        challenge=CHALLENGE,
        candidate_dir=case.candidate_dir,
        candidate_paths=case.candidate_paths,
        support_wheel_paths=case.support_paths,
        output_path=output,
        services=services,
        canonical_authority_bytes=case.canonical_authority_bytes,
    )

    data = written.read_bytes()
    payload = json.loads(data)
    assert data == canonical_json_bytes(payload)
    assert validate_public_evidence_tree("nvattest_proof", payload) == []
    assert (
        proof.validate_nvattest_proof_bytes(
            data,
            expected_challenge=CHALLENGE,
            target=case.target,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
            candidate_digest=CANDIDATE_DIGEST,
            ledger_sha256=LEDGER_SHA,
            canonical_authority_bytes=case.canonical_authority_bytes,
            expected_candidate_wheels=case.expected_candidate_wheels,
            expected_support_distributions=case.expected_support_distributions,
        )
        == []
    )
    text = data.decode("utf-8")
    assert payload["smoke"]["env"]["LD_LIBRARY_PATH"] in text
    for forbidden in ("/tmp", "/private", "/home", "site-packages", str(tmp_path)):
        assert forbidden not in text
    assert payload["smoke"]["argv"] == [
        f"{proof.NVATTEST_CACHE_ROOT}/bin/nvattest",
        "--help",
    ]
    assert payload["smoke"]["env"] == {
        **SCRUBBED_COMMAND_ENV,
        "LD_LIBRARY_PATH": (
            f"{proof.NVATTEST_CACHE_ROOT}/{NVATTEST_LIB_RELPATH.as_posix()}"
        ),
    }
    assert payload["cache_install"]["wheel_install_command"]["argv"][-8:] == [
        f"{proof.SUPPORT}/{entry['filename']}" for entry in case.support_distributions
    ]
    assert set(payload["cache_install"]["wheel_install_command"]) == {
        "argv",
        "env",
        "exit_code",
    }
    assert payload["cache_install"]["installed_closure"] == {
        "candidate": case.expected_candidate_wheels,
        "support": [
            {
                "metadata_sha256": entry["metadata_sha256"],
                "name": entry["name"],
                "version": entry["version"],
                "wheel": f"{proof.SUPPORT}/{entry['filename']}",
            }
            for entry in case.expected_support_distributions
        ],
    }


def test_wheel_install_evidence_is_cwd_invariant_when_pip_stdout_differs(
    tmp_path: Path,
) -> None:
    case = _synthetic_case(tmp_path, "linux-x86_64")
    receipt_bytes = []

    for label, install_stdout in (
        ("ancestor", PIP_ANCESTOR_STDOUT),
        ("unrelated", PIP_UNRELATED_STDOUT),
    ):
        output = tmp_path / f"{label}.json"
        proof.run_nvattest_proof(
            target=case.target,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
            candidate_digest=CANDIDATE_DIGEST,
            ledger_sha256=LEDGER_SHA,
            challenge=CHALLENGE,
            candidate_dir=case.candidate_dir,
            candidate_paths=case.candidate_paths,
            support_wheel_paths=case.support_paths,
            output_path=output,
            services=_synthetic_services(
                case,
                tmp_path,
                install_stdout=install_stdout,
            ),
            canonical_authority_bytes=case.canonical_authority_bytes,
        )
        data = output.read_bytes()
        receipt_bytes.append(data)
        assert (
            validate_public_evidence_text(f"{label}.receipt", data.decode("utf-8"))
            == []
        )

    assert receipt_bytes[0] == receipt_bytes[1]


def test_installed_closure_derivation_is_cwd_invariant(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    case = _synthetic_case(tmp_path, "linux-x86_64")
    nested = tmp_path / "nested" / "cwd"
    nested.mkdir(parents=True)
    outputs = []

    for cwd in (tmp_path, nested):
        monkeypatch.chdir(cwd)
        expected_candidate_wheels = proof.candidate_wheel_entries(case.candidate_paths)
        expected_support_distributions = (
            proof.support_distribution_entries_with_metadata(case.support_paths)
        )
        closure = proof._installed_closure_payload(
            _installed_distribution_observations(case),
            expected_candidate_wheels=expected_candidate_wheels,
            expected_support_distributions=expected_support_distributions,
        )
        outputs.append(canonical_json_bytes(closure))

    assert outputs[0] == outputs[1]


def test_installed_closure_rejects_different_candidate_wheel_metadata(
    tmp_path: Path,
) -> None:
    case = _synthetic_case(tmp_path, "linux-x86_64")
    receipt = _synthetic_receipt(case, tmp_path)
    replacement_dir = tmp_path / "replacement-candidate"
    replacement_dir.mkdir()
    replacement = _write_metadata_wheel(
        replacement_dir / case.candidate_paths[0].name,
        name="solstone",
        version=VERSION,
        extra_metadata="Summary: different metadata bytes\n",
    )

    failures = proof.validate_nvattest_proof(
        receipt,
        expected_challenge=CHALLENGE,
        target=case.target,
        version=VERSION,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
        candidate_digest=CANDIDATE_DIGEST,
        ledger_sha256=LEDGER_SHA,
        canonical_authority_payload=json.loads(case.canonical_authority_bytes),
        canonical_authority_sha256=hashlib.sha256(
            case.canonical_authority_bytes
        ).hexdigest(),
        expected_candidate_wheels=proof.candidate_wheel_entries((replacement,)),
        expected_support_distributions=case.expected_support_distributions,
    )

    assert any(
        failure.error
        == "nvattest proof installed closure candidate set is not bound to wheels"
        for failure in failures
    )


def test_installed_closure_rejects_missing_support_distribution(tmp_path: Path) -> None:
    case = _synthetic_case(tmp_path, "linux-x86_64")
    observed = [
        entry
        for entry in _installed_distribution_observations(case)
        if entry["name"] != "anyio"
    ]

    with pytest.raises(proof.NvattestProofError) as exc_info:
        proof.run_nvattest_proof(
            target=case.target,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
            candidate_digest=CANDIDATE_DIGEST,
            ledger_sha256=LEDGER_SHA,
            challenge=CHALLENGE,
            candidate_dir=case.candidate_dir,
            candidate_paths=case.candidate_paths,
            support_wheel_paths=case.support_paths,
            output_path=tmp_path / "missing-support.json",
            services=_synthetic_services(
                case,
                tmp_path,
                observed_distributions=observed,
            ),
            canonical_authority_bytes=case.canonical_authority_bytes,
        )

    assert any(
        failure.error == "nvattest installed closure is missing distribution"
        and failure.expected == "anyio"
        for failure in exc_info.value.failures
    )


def test_installed_distribution_observer_dedupes_lib64_dist_info_alias(
    tmp_path: Path,
) -> None:
    lib = tmp_path / "lib"
    lib.mkdir()
    dist_info = _write_installed_dist_info(lib, name="solstone", version=VERSION)
    lib64 = tmp_path / "lib64"
    lib64.symlink_to(lib, target_is_directory=True)

    observed = proof._default_observe_installed_distributions(
        _python_with_pythonpath(tmp_path, (lib, lib64))
    )

    assert [
        entry
        for entry in observed
        if proof._normalize_distribution_name(entry["name"]) == "solstone"
    ] == [
        {
            "metadata_sha256": hashlib.sha256(
                (dist_info / "METADATA").read_bytes()
            ).hexdigest(),
            "name": "solstone",
            "version": VERSION,
        }
    ]


def test_installed_closure_rejects_duplicate_distribution_realpaths(
    tmp_path: Path,
) -> None:
    one = tmp_path / "one"
    two = tmp_path / "two"
    one.mkdir()
    two.mkdir()
    first = _write_installed_dist_info(one, name="solstone", version=VERSION)
    _write_installed_dist_info(two, name="solstone", version=VERSION)
    observed = proof._default_observe_installed_distributions(
        _python_with_pythonpath(tmp_path, (one, two))
    )

    with pytest.raises(proof.NvattestProofError) as exc_info:
        proof._installed_closure_payload(
            observed,
            expected_candidate_wheels=(
                {
                    "metadata_sha256": hashlib.sha256(
                        (first / "METADATA").read_bytes()
                    ).hexdigest(),
                    "name": "solstone",
                    "version": VERSION,
                    "wheel": "CANDIDATE/solstone-1.0.0-py3-none-any.whl",
                    "wheel_bytes": 1,
                    "wheel_sha256": "0" * 64,
                },
            ),
            expected_support_distributions=(),
        )

    duplicate = next(
        failure
        for failure in exc_info.value.failures
        if failure.error == "nvattest installed closure distribution is duplicated"
    )
    assert duplicate.actual == "solstone"
    assert duplicate.repair == (
        "repair the proof environment so it contains one installed distribution "
        "named solstone, then regenerate the retained nvattest proof from the "
        "original release inputs"
    )


def test_installed_distribution_observer_reports_relevant_unreadable_metadata(
    tmp_path: Path,
) -> None:
    root = tmp_path / "site"
    root.mkdir()
    _write_installed_dist_info(root, name="solstone", version=VERSION, metadata=False)

    with pytest.raises(proof.NvattestProofError) as exc_info:
        proof._default_observe_installed_distributions(
            _python_with_pythonpath(tmp_path, (root,))
        )

    assert all(
        failure.error != "nvattest installed closure is missing distribution"
        for failure in exc_info.value.failures
    )
    failure = exc_info.value.failures[0]
    assert (
        failure.error
        == "nvattest installed distribution dist-info METADATA could not be read"
    )
    assert failure.expected == "readable dist-info METADATA for solstone"
    assert failure.repair == (
        "repair distribution solstone's dist-info METADATA so it can be read"
    )


def test_command_text_normalization_fails_closed_on_prefix_collision() -> None:
    env_root = Path("/tmp/abc")
    candidate_dir = Path("/tmp/candidate")
    cache_root = env_root / "journal" / "cache" / "providers" / "nvattest"
    collision = "/tmp/abcdef/x"
    child_path = "/tmp/abc/bin/python"
    result = proof.CommandResult(
        argv=(child_path,),
        exit_code=0,
        stdout=f"normalized {child_path}\nleaked {collision}",
        stderr=f"leaked {collision}",
        env=SCRUBBED_COMMAND_ENV,
    )

    payload = proof._command_payload(
        result,
        env_root=env_root,
        candidate_dir=candidate_dir,
        cache_root=cache_root,
        site_roots=(),
    )

    assert payload["argv"] == [f"{proof.ENVROOT}/bin/python"]
    assert f"{proof.ENVROOT}/bin/python" in payload["stdout"]
    assert "ENVROOTdef" not in payload["stdout"]
    assert collision in payload["stdout"]
    assert collision in payload["stderr"]
    assert {
        failure.error
        for failure in validate_public_evidence_tree("nvattest_proof", payload)
    } == {
        "nvattest_proof.stderr contains disallowed content",
        "nvattest_proof.stdout contains disallowed content",
    }


def test_default_run_smoke_uses_payload_library_path_offline(tmp_path: Path) -> None:
    root = tmp_path / "nvattest"
    bin_dir = root / "bin"
    lib_dir = root / "lib"
    bin_dir.mkdir(parents=True)
    lib_dir.mkdir()
    nvattest_bin = bin_dir / "nvattest"
    nvattest_bin.write_text(
        "#!/bin/sh\n"
        f'if [ "${{LD_LIBRARY_PATH:-}}" != "{lib_dir}" ]; then\n'
        '  echo "error while loading shared libraries: libnvat.so.1: '
        'cannot open shared object file: No such file or directory" >&2\n'
        "  exit 127\n"
        "fi\n"
        "exit 0\n",
        encoding="utf-8",
    )
    nvattest_bin.chmod(0o755)

    result = proof._default_run_smoke(root, nvattest_bin)

    assert result.exit_code == 0
    assert result.env == {
        **SCRUBBED_COMMAND_ENV,
        "LD_LIBRARY_PATH": str(lib_dir),
    }


@pytest.mark.integration
@pytest.mark.release
def test_default_run_smoke_uses_payload_library_path_for_real_linux_archive(
    tmp_path: Path,
) -> None:
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        pytest.skip("real nvattest linux-x86_64 smoke requires linux/x86_64")
    target_key = "linux-x86_64"
    entry = authority_entry(target_key)
    archive = download_real_archive(tmp_path / "downloads", entry)
    authority_json = json.loads(
        Path(nvattest_authority.__file__)
        .with_name("nvattest_authority_v1.json")
        .read_text(encoding="utf-8")
    )
    expected_sha = authority_json["targets"][target_key]["artifact"]["sha256"]
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected_sha
    raw_dir = tmp_path / "extract"
    nvattest_install._safe_extract_nvattest_tarball(archive, raw_dir)
    root = nvattest_install._find_extracted_root(raw_dir, entry)

    result = proof._default_run_smoke(root, root / "bin" / "nvattest")

    assert result.exit_code == 0
    assert result.env == {
        **SCRUBBED_COMMAND_ENV,
        "LD_LIBRARY_PATH": str(root / "lib"),
    }


@pytest.mark.parametrize(
    ("label", "mutate"),
    [
        ("challenge", lambda receipt: receipt.update({"challenge": "0" * 64})),
        ("target", lambda receipt: receipt.update({"target": "macos-arm64"})),
        ("version", lambda receipt: receipt.update({"version": "9.9.9"})),
        (
            "source_commit",
            lambda receipt: receipt.update({"source_commit": "f" * 40}),
        ),
        (
            "candidate_digest",
            lambda receipt: receipt.update({"candidate_digest": "1" * 64}),
        ),
        (
            "core_lock_sha256",
            lambda receipt: receipt.update({"core_lock_sha256": "2" * 64}),
        ),
        ("ledger_sha256", lambda receipt: receipt.update({"ledger_sha256": "3" * 64})),
        (
            "authority_digest",
            lambda receipt: receipt["installed_authority"].update({"sha256": "4" * 64}),
        ),
        (
            "archive_hash",
            lambda receipt: receipt["archive_fetch"].update({"sha256": "5" * 64}),
        ),
        (
            "manifest_hash",
            lambda receipt: receipt["manifest_fetch"].update({"sha256": "6" * 64}),
        ),
        (
            "smoke_argv",
            lambda receipt: receipt["smoke"].update({"argv": ["nvattest", "--help"]}),
        ),
        (
            "smoke_env",
            lambda receipt: receipt["smoke"].update(
                {
                    "env": {
                        **SCRUBBED_COMMAND_ENV,
                        "LD_LIBRARY_PATH": "NVATTEST_CACHE_ROOT/not-lib",
                    }
                }
            ),
        ),
        ("smoke_exit", lambda receipt: receipt["smoke"].update({"exit_code": 1})),
    ],
)
def test_validator_rejects_bound_field_mutations(
    tmp_path: Path,
    label: str,
    mutate: Callable[[dict[str, Any]], None],
) -> None:
    del label
    case = _synthetic_case(tmp_path, "linux-x86_64")
    receipt = _synthetic_receipt(case, tmp_path)

    mutate(receipt)

    assert proof.validate_nvattest_proof(
        receipt,
        expected_challenge=CHALLENGE,
        target=case.target,
        version=VERSION,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
        candidate_digest=CANDIDATE_DIGEST,
        ledger_sha256=LEDGER_SHA,
        canonical_authority_payload=json.loads(case.canonical_authority_bytes),
        canonical_authority_sha256=hashlib.sha256(
            case.canonical_authority_bytes
        ).hexdigest(),
        expected_candidate_wheels=case.expected_candidate_wheels,
        expected_support_distributions=case.expected_support_distributions,
    )


@pytest.mark.parametrize(
    ("label", "mutate"),
    [
        ("missing", lambda entries: entries.pop()),
        ("extra", lambda entries: entries.append({**entries[0], "name": "bogus"})),
        ("duplicate", lambda entries: entries.append(dict(entries[0]))),
        ("reordered", lambda entries: entries.reverse()),
        ("mismatched", lambda entries: entries[0].update({"sha256": "0" * 64})),
    ],
)
def test_validator_rejects_support_declaration_mutations(
    tmp_path: Path,
    label: str,
    mutate: Callable[[list[dict[str, Any]]], None],
) -> None:
    del label
    case = _synthetic_case(tmp_path, "linux-x86_64")
    receipt = _synthetic_receipt(case, tmp_path)
    mutated = [dict(entry) for entry in receipt["support_distributions"]]
    mutate(mutated)
    receipt["support_distributions"] = mutated

    failures = proof.validate_nvattest_proof(
        receipt,
        expected_challenge=CHALLENGE,
        target=case.target,
        version=VERSION,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
        candidate_digest=CANDIDATE_DIGEST,
        ledger_sha256=LEDGER_SHA,
        canonical_authority_payload=json.loads(case.canonical_authority_bytes),
        canonical_authority_sha256=hashlib.sha256(
            case.canonical_authority_bytes
        ).hexdigest(),
        expected_candidate_wheels=case.expected_candidate_wheels,
        expected_support_distributions=case.expected_support_distributions,
    )
    assert failures


@pytest.mark.parametrize(
    ("label", "paths_mutator"),
    [
        ("missing", lambda paths, extra: paths[:-1]),
        ("extra", lambda paths, extra: (*paths, extra)),
        ("duplicate", lambda paths, extra: (*paths, paths[0])),
    ],
)
def test_support_wheel_input_rejects_missing_extra_and_duplicate_sets(
    tmp_path: Path,
    label: str,
    paths_mutator: Callable[[tuple[Path, ...], Path], Sequence[Path]],
) -> None:
    del label
    support_dir = tmp_path / "support"
    support_dir.mkdir()
    support_paths = _write_support_wheels(support_dir)
    extra = _write_metadata_wheel(
        support_dir / "urllib3-2.0.0-py3-none-any.whl",
        name="urllib3",
        version="2.0.0",
    )

    with pytest.raises(proof.NvattestProofError):
        proof.support_distribution_entries(paths_mutator(support_paths, extra))


@pytest.mark.parametrize(
    ("label", "mutate"),
    [
        ("schema", lambda manifest: manifest.update({"schema_version": 1})),
        ("version", lambda manifest: manifest["release"].update({"version": "bad"})),
        ("target", lambda manifest: manifest["target"].update({"id": "bad"})),
        ("fork", lambda manifest: manifest["source"].update({"commit": "bad"})),
        (
            "upstream",
            lambda manifest: manifest["source"].update({"upstream_base_commit": "bad"}),
        ),
        (
            "artifact",
            lambda manifest: manifest["artifact"].update({"sha256": "0" * 64}),
        ),
        (
            "inventory",
            lambda manifest: manifest["archive_members"].pop(),
        ),
    ],
)
def test_companion_manifest_semantic_validator_rejects_mutations(
    tmp_path: Path,
    label: str,
    mutate: Callable[[dict[str, Any]], None],
) -> None:
    del label
    case = _synthetic_case(tmp_path, "linux-x86_64")
    manifest = json.loads(case.manifest_path.read_bytes())
    mutate(manifest)

    failures = proof.validate_companion_manifest_bytes(
        canonical_json_bytes(manifest),
        target_key=case.target_key,
        authority_target=case.authority_target,
    )
    assert failures


@pytest.mark.parametrize(
    ("target", "host", "expected"),
    [
        (
            "linux-x86_64-musl",
            proof.HostObservation(os="Linux", arch="x86_64"),
            "linux-x86_64",
        ),
        (
            "linux-aarch64-musl",
            proof.HostObservation(os="Linux", arch="arm64"),
            "linux-aarch64",
        ),
        (
            "macos-arm64",
            proof.HostObservation(os="Darwin", arch="arm64"),
            "macos-arm64",
        ),
    ],
)
def test_host_policy_derives_target_key_without_second_table(
    target: str,
    host: proof.HostObservation,
    expected: str,
) -> None:
    assert proof._target_key_from_policy(target, host) == expected


@pytest.mark.parametrize(
    "host",
    [
        proof.HostObservation(os="Darwin", arch="x86_64"),
        proof.HostObservation(os="Linux", arch="armv7"),
    ],
)
def test_spoofed_or_near_miss_host_fails_before_reach(
    tmp_path: Path,
    host: proof.HostObservation,
) -> None:
    calls: list[str] = []
    services = proof.NvattestProofServices(
        create_environment=lambda _target: calls.append("environment") or tmp_path,
        install_wheels=lambda *_args: (
            calls.append("install") or _command_result(("python",))
        ),
        fetch=lambda *_args: (
            calls.append("fetch") or pytest.fail("fetch should not run")
        ),
        run_package_install=lambda *_args: (
            calls.append("driver") or pytest.fail("driver should not run")
        ),
        observe_installed_distributions=lambda *_args: pytest.fail(
            "installed closure should not run"
        ),
        integrity_recheck=lambda *_args: {},
        run_smoke=lambda _root, _path: pytest.fail("smoke should not run"),
        clock=lambda: RECORDED_AT,
        cleanup=lambda _path: calls.append("cleanup"),
        observe_host=lambda: host,
    )

    with pytest.raises(proof.NvattestProofError) as exc_info:
        proof.run_nvattest_proof(
            target="macos-arm64",
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
            candidate_digest=CANDIDATE_DIGEST,
            ledger_sha256=LEDGER_SHA,
            challenge=CHALLENGE,
            candidate_dir=tmp_path,
            candidate_paths=(),
            support_wheel_paths=(),
            output_path=tmp_path / "proof.json",
            services=services,
        )

    assert [failure.error for failure in exc_info.value.failures] == ["host-validation"]
    assert calls == []


def test_receipt_validator_names_policy_negative_cases(tmp_path: Path) -> None:
    case = _synthetic_case(tmp_path, "linux-x86_64")
    receipt = _synthetic_receipt(case, tmp_path)

    mutations = [
        lambda data: data["cache_install"]["wheel_install_command"]["argv"].append(
            "--find-links"
        ),
        lambda data: data["cache_install"]["wheel_install_command"]["argv"].append(
            "--index-url"
        ),
        lambda data: data["cache_install"]["wheel_install_command"].update(
            {"exit_code": 1}
        ),
        lambda data: data["cache_install"]["wheel_install_command"].update(
            {"stdout": PIP_ANCESTOR_STDOUT}
        ),
        lambda data: data["installed_package"].update(
            {
                "module_origin": "/workspace/owner/source/solstone/think/providers/nvattest_install.py"
            }
        ),
        lambda data: data["smoke"].update({"argv": ["nvattest", "--help"]}),
        lambda data: data["smoke"].update(
            {
                "env": {
                    **SCRUBBED_COMMAND_ENV,
                    "LD_LIBRARY_PATH": "NVATTEST_CACHE_ROOT/not-lib",
                }
            }
        ),
        lambda data: data["support_distributions"].pop(),
    ]
    for mutate in mutations:
        mutated = copy.deepcopy(receipt)
        mutate(mutated)
        assert proof.validate_nvattest_proof(
            mutated,
            expected_challenge=CHALLENGE,
            target=case.target,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
            candidate_digest=CANDIDATE_DIGEST,
            ledger_sha256=LEDGER_SHA,
            canonical_authority_payload=json.loads(case.canonical_authority_bytes),
            canonical_authority_sha256=hashlib.sha256(
                case.canonical_authority_bytes
            ).hexdigest(),
            expected_candidate_wheels=case.expected_candidate_wheels,
            expected_support_distributions=case.expected_support_distributions,
        )


def _synthetic_receipt(case: SyntheticCase, tmp_path: Path) -> dict[str, Any]:
    output = tmp_path / f"{case.target_key}-receipt.json"
    proof.run_nvattest_proof(
        target=case.target,
        version=VERSION,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
        candidate_digest=CANDIDATE_DIGEST,
        ledger_sha256=LEDGER_SHA,
        challenge=CHALLENGE,
        candidate_dir=case.candidate_dir,
        candidate_paths=case.candidate_paths,
        support_wheel_paths=case.support_paths,
        output_path=output,
        services=_synthetic_services(case, tmp_path),
        canonical_authority_bytes=case.canonical_authority_bytes,
    )
    return json.loads(output.read_bytes())


def _synthetic_case(tmp_path: Path, target_key: NvattestTargetKey) -> SyntheticCase:
    target = PROOF_TARGET_BY_KEY[target_key]
    candidate_dir = tmp_path / f"candidate-{target_key}"
    candidate_dir.mkdir()
    candidate_paths = (
        _write_metadata_wheel(
            candidate_dir / "solstone-1.0.0-py3-none-any.whl",
            name="solstone",
            version=VERSION,
        ),
    )
    support_dir = tmp_path / f"support-{target_key}"
    support_dir.mkdir()
    support_paths = _write_support_wheels(support_dir)
    expected_candidate_wheels = proof.candidate_wheel_entries(candidate_paths)
    expected_support_distributions = proof.support_distribution_entries_with_metadata(
        support_paths
    )
    support_distributions = proof.support_distribution_entries(support_paths)

    entry = authority_entry(target_key)
    archive_path = tmp_path / f"{target_key}.tar.xz"
    _write_payload_tarball(
        archive_path,
        entry,
        roots=("",),
        label=target_key,
        omitted=set(),
        executable_overrides={},
    )
    archive_bytes = archive_path.read_bytes()
    payload = copy.deepcopy(authority_payload())
    authority_target = payload["targets"][target_key]
    authority_target["artifact"]["size_bytes"] = len(archive_bytes)
    authority_target["artifact"]["sha256"] = hashlib.sha256(archive_bytes).hexdigest()
    manifest = _synthetic_manifest(target_key, authority_target)
    manifest_bytes = canonical_json_bytes(manifest)
    manifest_path = tmp_path / authority_target["companion_manifest"]["name"]
    manifest_path.write_bytes(manifest_bytes)
    authority_target["companion_manifest"]["sha256"] = hashlib.sha256(
        manifest_bytes
    ).hexdigest()
    manifest_path.write_bytes(
        canonical_json_bytes(_synthetic_manifest(target_key, authority_target))
    )
    canonical_authority_bytes = (
        json.dumps(payload, indent=2, sort_keys=True).encode("utf-8") + b"\n"
    )
    return SyntheticCase(
        target_key=target_key,
        target=target,
        candidate_dir=candidate_dir,
        candidate_paths=candidate_paths,
        support_paths=support_paths,
        expected_candidate_wheels=expected_candidate_wheels,
        expected_support_distributions=expected_support_distributions,
        support_distributions=support_distributions,
        archive_path=archive_path,
        manifest_path=manifest_path,
        canonical_authority_bytes=canonical_authority_bytes,
        authority_target=authority_target,
    )


def _synthetic_manifest(
    target_key: NvattestTargetKey,
    authority_target: Mapping[str, Any],
) -> dict[str, Any]:
    source = authority_target["source"]
    artifact = authority_target["artifact"]
    inventory = authority_target["inventory"]
    symlinks = [member for member in inventory if member["kind"] == "symlink"]
    regular = [
        member
        for member in inventory
        if member["kind"] == "regular" and member["relpath"] != "bin/nvattest"
    ]
    member_order = [
        inventory[0],
        *sorted(symlinks, key=lambda item: item["relpath"]),
        *regular,
    ]
    return {
        "archive_members": [
            {
                "kind": member["kind"],
                "link_target": member["symlink_target"],
                "path": member["relpath"],
            }
            for member in member_order
        ],
        "artifact": {
            "name": artifact["name"],
            "sha256": artifact["sha256"],
            "size": artifact["size_bytes"],
        },
        "build_inputs": {"ignored": True},
        "build_tools": {"ignored": True},
        "dependency_pins": [],
        "release": {"sol_revision": "synthetic", "version": source["version"]},
        "schema_version": 2,
        "source": {
            "commit": source["fork_commit"],
            "sol_series_commits": [],
            "source_date_epoch": 0,
            "upstream_base_commit": source["upstream_base"],
        },
        "target": {
            "abi": "synthetic",
            "architecture": TARGET_POLICY[PROOF_TARGET_BY_KEY[target_key]][1],
            "binary_format": "synthetic",
            "id": target_key,
        },
    }


def _synthetic_services(
    case: SyntheticCase,
    tmp_path: Path,
    *,
    install_stdout: str = PIP_ANCESTOR_STDOUT,
    observed_distributions: Sequence[Mapping[str, Any]] | None = None,
) -> proof.NvattestProofServices:
    env_root = tmp_path / f"env-{case.target_key}"
    policy_os, policy_arch = TARGET_POLICY[case.target]
    installed_observations = tuple(
        dict(entry)
        for entry in (
            observed_distributions
            if observed_distributions is not None
            else _installed_distribution_observations(case)
        )
    )

    def create_environment(_target: str) -> Path:
        (env_root / "bin").mkdir(parents=True)
        python = env_root / "bin" / "python"
        python.write_text("#!/bin/sh\nprintf 'solstone==1.0.0\\n'\n", encoding="utf-8")
        python.chmod(0o755)
        return env_root

    def install_wheels(
        env_python: Path,
        candidate_wheels: Sequence[Path],
        support_wheels: Sequence[Path],
    ) -> proof.CommandResult:
        assert tuple(candidate_wheels) == case.candidate_paths
        assert tuple(support_wheels) == case.support_paths
        site_root = _site_root(env_root)
        authority_path = (
            site_root
            / "solstone"
            / "think"
            / "providers"
            / "nvattest_authority_v1.json"
        )
        authority_path.parent.mkdir(parents=True)
        authority_path.write_bytes(case.canonical_authority_bytes)
        (site_root / "solstone-1.0.0.dist-info").mkdir()
        return _command_result(
            (
                str(env_python),
                "-m",
                "pip",
                "install",
                "--no-index",
                "--no-deps",
                *(str(path) for path in candidate_wheels),
                *(str(path) for path in case.support_paths),
            ),
            stdout=install_stdout,
        )

    def fetch(label: str, url: str, dest: Path) -> proof.FetchObservation:
        source = case.archive_path if label == "archive" else case.manifest_path
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
        sha256, size_bytes = proof.file_sha256_size(dest)
        return proof.FetchObservation(
            label=label,
            url=url,
            path=dest,
            sha256=sha256,
            size_bytes=size_bytes,
        )

    def run_package_install(
        env_python: Path,
        driver_path: Path,
        target_key: str,
        journal_path: Path,
    ) -> proof.DriverObservation:
        assert target_key == case.target_key
        payload = _driver_payload(case, env_root, journal_path)
        return proof.DriverObservation(
            command=_command_result(
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

    def run_smoke(nvattest_root: Path, nvattest_bin: Path) -> proof.CommandResult:
        return _command_result(
            (str(nvattest_bin), "--help"),
            stdout="usage\n",
            env={**SCRUBBED_COMMAND_ENV, **nvattest_library_env(nvattest_root)},
        )

    def observe_installed_distributions(
        _env_python: Path,
    ) -> Sequence[Mapping[str, Any]]:
        return [dict(entry) for entry in installed_observations]

    return proof.NvattestProofServices(
        create_environment=create_environment,
        install_wheels=install_wheels,
        fetch=fetch,
        run_package_install=run_package_install,
        observe_installed_distributions=observe_installed_distributions,
        integrity_recheck=lambda _journal, _target, _fetches, driver: {
            "members": driver.payload["members"],
            "sidecar": driver.payload["sidecar"],
            "sidecar_path": driver.payload["sidecar_path"],
            "sidecar_sha256": driver.payload["sidecar_sha256"],
            "sidecar_size_bytes": driver.payload["sidecar_size_bytes"],
            "tree_fingerprint_sha256": driver.payload["tree_fingerprint_sha256"],
        },
        run_smoke=run_smoke,
        clock=lambda: RECORDED_AT,
        cleanup=lambda path: shutil.rmtree(path),
        observe_host=lambda: proof.HostObservation(os=policy_os, arch=policy_arch),
    )


def _installed_distribution_observations(
    case: SyntheticCase,
) -> list[dict[str, Any]]:
    return sorted(
        [
            {
                "metadata_sha256": entry["metadata_sha256"],
                "name": entry["name"],
                "version": entry["version"],
            }
            for entry in (
                *case.expected_candidate_wheels,
                *case.expected_support_distributions,
            )
        ],
        key=lambda item: item["name"],
    )


def _driver_payload(
    case: SyntheticCase,
    env_root: Path,
    journal_path: Path,
) -> dict[str, Any]:
    site_root = _site_root(env_root)
    authority_path = (
        site_root / "solstone" / "think" / "providers" / "nvattest_authority_v1.json"
    )
    cache_root = journal_path / "cache" / "providers" / "nvattest"
    sidecar_path = cache_root / ".nvattest-install.json"
    fingerprint = "f" * 64
    sidecar = {
        "artifact": dict(case.authority_target["artifact"]),
        "schema_version": SIDECAR_SCHEMA_VERSION,
        "target_key": case.target_key,
        "tree_fingerprint_sha256": fingerprint,
        "version": case.authority_target["source"]["version"],
    }
    sidecar_bytes = canonical_json_bytes(sidecar)
    return {
        "authority_module_file": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_authority.py"
        ),
        "authority_origin": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_authority.py"
        ),
        "authority_path": str(authority_path),
        "authority_sha256": hashlib.sha256(case.canonical_authority_bytes).hexdigest(),
        "authority_size_bytes": len(case.canonical_authority_bytes),
        "cache_root": str(cache_root),
        "dist_info": [
            {
                "dist_info_path": str(site_root / "solstone-1.0.0.dist-info"),
                "name": "solstone",
                "version": VERSION,
            }
        ],
        "journal_path": str(journal_path),
        "members": _member_facts(case.authority_target),
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


def _member_facts(authority_target: Mapping[str, Any]) -> list[dict[str, Any]]:
    return [
        {
            "content_sha256": (
                hashlib.sha256(member["relpath"].encode("utf-8")).hexdigest()
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


def _site_root(env_root: Path) -> Path:
    return env_root / "lib" / "python3.13" / "site-packages"


def _write_support_wheels(path: Path) -> tuple[Path, ...]:
    return tuple(
        _write_metadata_wheel(
            path / f"{name.replace('-', '_')}-{version}-py3-none-any.whl",
            name=name,
            version=version,
        )
        for name, version in sorted(SUPPORT_VERSIONS.items())
    )


def _python_with_pythonpath(tmp_path: Path, roots: Sequence[Path]) -> Path:
    wrapper = tmp_path / "python-with-pythonpath"
    cwd = tmp_path / "python-cwd"
    cwd.mkdir()
    pythonpath = os.pathsep.join(str(root) for root in roots)
    wrapper.write_text(
        "\n".join(
            (
                "#!/bin/sh",
                f"cd {shlex.quote(str(cwd))}",
                (
                    f"PYTHONPATH={shlex.quote(pythonpath)} "
                    f'exec {shlex.quote(sys.executable)} -S "$@"'
                ),
                "",
            )
        ),
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    return wrapper


def _write_installed_dist_info(
    root: Path,
    *,
    name: str,
    version: str,
    metadata: bool = True,
) -> Path:
    dist_info = root / f"{name.replace('-', '_')}-{version}.dist-info"
    dist_info.mkdir()
    if metadata:
        (dist_info / "METADATA").write_text(
            f"Name: {name}\nVersion: {version}\n",
            encoding="utf-8",
        )
    return dist_info


def _write_metadata_wheel(
    path: Path,
    *,
    name: str,
    version: str,
    extra_metadata: str = "",
) -> Path:
    dist_info = f"{name.replace('-', '_')}-{version}.dist-info"
    with zipfile.ZipFile(path, "w") as wheel:
        wheel.writestr(
            f"{dist_info}/METADATA",
            f"Name: {name}\nVersion: {version}\n{extra_metadata}",
        )
    return path


def _command_result(
    argv: Sequence[str],
    *,
    stdout: str = "",
    exit_code: int = 0,
    env: Mapping[str, str] = SCRUBBED_COMMAND_ENV,
) -> proof.CommandResult:
    return proof.CommandResult(
        argv=tuple(argv),
        exit_code=exit_code,
        stdout=stdout,
        stderr="",
        env=env,
    )


def test_default_fetch_sends_an_explicit_user_agent(tmp_path, monkeypatch):
    """The artifact edge answers 403 to the anonymous Python-urllib agent.

    Every other test in this module injects a fake fetch, so the real
    _default_fetch path is the one nothing exercised. Pin the header here.
    """
    import io
    import urllib.request

    captured: dict[str, object] = {}

    class _Response(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            self.close()
            return False

    def fake_urlopen(request, timeout=None):
        captured["request"] = request
        captured["timeout"] = timeout
        return _Response(b"payload-bytes")

    monkeypatch.setattr(urllib.request, "urlopen", fake_urlopen)

    dest = tmp_path / "archive.tar.xz"
    observation = proof._default_fetch("archive", "https://example.invalid/a", dest)

    request = captured["request"]
    assert isinstance(request, urllib.request.Request)
    agent = request.get_header("User-agent")
    assert agent == proof.NVATTEST_PROOF_USER_AGENT
    assert agent and not agent.lower().startswith("python-urllib")
    assert dest.read_bytes() == b"payload-bytes"
    assert observation.size_bytes == len(b"payload-bytes")
