# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import zipfile
from dataclasses import replace
from pathlib import Path

import pytest

import scripts.record_macos_native_wheel as native
import scripts.release_tool_pins as pins
from scripts.check_wheel_contents import (
    CORE_SCRIPT_NAMES,
    PARAKEET_HELPER_MEMBER,
    SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR,
    SPEAKERS_ANALYZE_SCRIPT_NAMES,
    SPEAKERS_ANALYZE_TARGETS,
)

SOURCE_COMMIT = "a" * 40
CORE_LOCK = "b" * 64


def _write_member_wheel(path: Path, member: str, content: bytes) -> None:
    info = zipfile.ZipInfo(member)
    info.create_system = 3
    info.external_attr = 0o755 << 16
    with zipfile.ZipFile(path, "w") as wheel:
        wheel.writestr(info, content)


def _root_wheel(tmp_path: Path, content: bytes = b"root-helper") -> Path:
    path = tmp_path / "solstone-1.2.3-py3-none-macosx_14_0_arm64.whl"
    _write_member_wheel(path, PARAKEET_HELPER_MEMBER, content)
    return path


def _core_wheel(tmp_path: Path, content: bytes = b"core-script") -> Path:
    path = tmp_path / "solstone_core-1.2.3-py3-none-macosx_14_0_arm64.whl"
    with zipfile.ZipFile(path, "w") as wheel:
        for name in CORE_SCRIPT_NAMES:
            info = zipfile.ZipInfo(f"solstone_core-1.2.3.data/scripts/{name}")
            info.create_system = 3
            info.external_attr = 0o755 << 16
            wheel.writestr(info, content)
    return path


def _speakers_analyze_wheel(
    tmp_path: Path,
    *,
    script: bytes = b"speakers-script",
    dylib: bytes = b"onnxruntime-dylib",
) -> Path:
    path = (
        tmp_path / "solstone_core_speakers_analyze-1.2.3-py3-none-macosx_14_0_arm64.whl"
    )
    data_prefix = "solstone_core_speakers_analyze-1.2.3.data"
    dylib_name = SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name
    with zipfile.ZipFile(path, "w") as wheel:
        for member, content in (
            (
                f"{data_prefix}/scripts/{SPEAKERS_ANALYZE_SCRIPT_NAMES[0]}",
                script,
            ),
            (
                f"{data_prefix}/{SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR.as_posix()}/{dylib_name}",
                dylib,
            ),
        ):
            info = zipfile.ZipInfo(member)
            info.create_system = 3
            info.external_attr = 0o755 << 16
            wheel.writestr(info, content)
    return path


def _facts(content: bytes) -> dict:
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


def _facts_file(tmp_path: Path, facts: dict) -> Path:
    path = tmp_path / "facts.json"
    path.write_text(json.dumps(facts), encoding="utf-8")
    return path


def _patch_macos_runtime_pin(monkeypatch: pytest.MonkeyPatch, content: bytes) -> None:
    spec = native.SPEAKERS_ANALYZE_TARGETS["macos-arm64"]
    monkeypatch.setitem(
        native.SPEAKERS_ANALYZE_TARGETS,
        "macos-arm64",
        replace(spec, runtime_sha256=hashlib.sha256(content).hexdigest()),
    )


def _core_facts(content: bytes) -> dict:
    return {"members": {name: _facts(content) for name in CORE_SCRIPT_NAMES}}


def _speakers_analyze_facts(script: bytes, dylib: bytes) -> dict:
    return {
        "members": {
            SPEAKERS_ANALYZE_SCRIPT_NAMES[0]: _facts(script),
            SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name: _facts(dylib),
        }
    }


@pytest.mark.release
def test_native_record_cli_and_makefile_use_package_module() -> None:
    root = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        [sys.executable, "-m", "scripts.record_macos_native_wheel", "--help"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stderr
    makefile = (root / "Makefile").read_text(encoding="utf-8")
    assert makefile.count("python3 -m scripts.record_macos_native_wheel") == 1
    assert "python3 scripts/build_macos_release_packages.py" in makefile
    assert "python3 scripts/record_macos_native_wheel.py" not in makefile


def test_exactly_three_role_records_are_written_and_not_interchangeable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _patch_macos_runtime_pin(monkeypatch, b"dylib")
    root_wheel = _root_wheel(tmp_path, b"root")
    core_wheel = _core_wheel(tmp_path, b"core")
    speakers_wheel = _speakers_analyze_wheel(
        tmp_path,
        script=b"speakers",
        dylib=b"dylib",
    )

    root = native.build_macos_native_record(
        role="root",
        wheel_path=root_wheel,
        signing_facts=_facts(b"root"),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    core = native.build_macos_native_record(
        role="core",
        wheel_path=core_wheel,
        signing_facts=_core_facts(b"core"),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    speakers = native.build_macos_native_record(
        role="speakers-analyze",
        wheel_path=speakers_wheel,
        signing_facts=_speakers_analyze_facts(b"speakers", b"dylib"),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )

    assert {root["role"], core["role"], speakers["role"]} == {
        "root",
        "core",
        "speakers-analyze",
    }
    assert set(speakers["members"]) == {
        SPEAKERS_ANALYZE_SCRIPT_NAMES[0],
        SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name,
    }
    assert set(speakers["unsigned_members"]) == set(speakers["members"])
    assert native.validate_macos_native_record(
        root,
        role="core",
        wheel_path=core_wheel,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    assert native.validate_macos_native_record(
        core,
        role="root",
        wheel_path=root_wheel,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    assert native.validate_macos_native_record(
        speakers,
        role="root",
        wheel_path=root_wheel,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )


def test_record_rejects_member_hash_signing_and_notary_mismatches(
    tmp_path: Path,
) -> None:
    wheel = _root_wheel(tmp_path, b"root")
    facts = _facts(b"different")

    with pytest.raises(native.NativeRecordError) as exc:
        native.build_macos_native_record(
            role="root",
            wheel_path=wheel,
            signing_facts=facts,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
        )
    assert any(
        failure.error == "macOS signed binary hash does not match final wheel member"
        for failure in exc.value.failures
    )

    facts = _facts(b"root")
    facts["signer_pinned"] = False
    facts["team_pinned"] = False
    facts["notarization_status"] = "rejected"
    with pytest.raises(native.NativeRecordError) as exc:
        native.build_macos_native_record(
            role="root",
            wheel_path=wheel,
            signing_facts=facts,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
        )
    errors = {failure.error for failure in exc.value.failures}
    assert "macOS signing fact signer_pinned is not pinned true" in errors
    assert "macOS signing fact team_pinned is not pinned true" in errors
    assert "macOS notarization status is not accepted" in errors


def test_swift_abbreviated_and_suffix_skewed_evidence_is_rejected(
    tmp_path: Path,
) -> None:
    wheel = _root_wheel(tmp_path, b"root")
    for value in (
        "6.3.3",
        "swift 6.3.3",
        "Apple Swift 6.3.3",
        "Apple Swift 6.3.3 (swiftlang-6.3.3.1.3 clang-2100.1.1.102)",
        "Apple Swift 6.3.3 (swiftlang-6.3.3.1.4 clang-2100.1.1.101)",
        pins.MACOS_SWIFT_FIXTURE_BANNER.replace(
            "Apple Swift version 6.3.3", "Apple Swift version 6.3.4"
        ),
        pins.MACOS_SWIFT_FIXTURE_BANNER.replace(
            "swiftlang-6.3.3.1.3", "swiftlang-6.3.3.1.4"
        ),
        pins.MACOS_SWIFT_FIXTURE_BANNER.replace(
            "clang-2100.1.1.101", "clang-2100.1.1.102"
        ),
    ):
        facts = _facts(b"root")
        facts["tools"]["swift"] = value
        with pytest.raises(native.NativeRecordError) as exc:
            native.build_macos_native_record(
                role="root",
                wheel_path=wheel,
                signing_facts=facts,
                source_commit=SOURCE_COMMIT,
                core_lock_sha256=CORE_LOCK,
            )
        assert any(
            failure.error == "macOS swift tool evidence is not pinned"
            for failure in exc.value.failures
        )


@pytest.mark.parametrize(
    "value",
    (
        "notarytool 1.1.2 (41)",
        "1.1.3 (41)",
        "1.1.2 (42)",
    ),
)
def test_notarytool_prefixed_and_wrong_evidence_is_rejected(
    tmp_path: Path,
    value: str,
) -> None:
    wheel = _root_wheel(tmp_path, b"root")
    facts = _facts(b"root")
    facts["tools"]["notarytool"] = value

    with pytest.raises(native.NativeRecordError) as exc:
        native.build_macos_native_record(
            role="root",
            wheel_path=wheel,
            signing_facts=facts,
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
        )
    assert any(
        failure.error == "macOS notarytool tool evidence is not pinned"
        for failure in exc.value.failures
    )


@pytest.mark.parametrize(
    ("key", "value", "error"),
    (
        (
            "swift",
            "Apple Swift 6.3.3 (swiftlang-6.3.3.1.3 clang-2100.1.1.101)",
            "macOS native record tool swift is not pinned",
        ),
        (
            "notarytool",
            "notarytool 1.1.2 (41)",
            "macOS native record tool notarytool is not pinned",
        ),
    ),
)
def test_native_record_validation_rejects_swift_and_notarytool_skew(
    tmp_path: Path,
    key: str,
    value: str,
    error: str,
) -> None:
    wheel = _root_wheel(tmp_path, b"root")
    record = native.build_macos_native_record(
        role="root",
        wheel_path=wheel,
        signing_facts=_facts(b"root"),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    record["tools"][key] = value

    failures = native.validate_macos_native_record(
        record,
        role="root",
        wheel_path=wheel,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )

    assert any(failure.error == error for failure in failures)


def test_record_validation_rejects_wheel_hash_and_repacked_core_mismatch(
    tmp_path: Path,
) -> None:
    wheel = _core_wheel(tmp_path, b"core")
    record = native.build_macos_native_record(
        role="core",
        wheel_path=wheel,
        signing_facts=_core_facts(b"core"),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    record["wheel"]["sha256"] = "0" * 64

    failures = native.validate_macos_native_record(
        record,
        role="core",
        wheel_path=wheel,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )

    assert any(
        failure.error == "macOS native record wheel does not match final wheel"
        for failure in failures
    )


def test_record_writer_removes_atomic_temp_on_success_and_failure(
    tmp_path: Path,
) -> None:
    wheel = _root_wheel(tmp_path, b"root")
    output = tmp_path / "record.json"
    native.write_macos_native_record(
        role="root",
        wheel_path=wheel,
        signing_facts_path=_facts_file(tmp_path, _facts(b"root")),
        output_path=output,
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=CORE_LOCK,
    )
    assert output.exists()
    assert not (tmp_path / ".record.json.tmp").exists()

    with pytest.raises(native.NativeRecordError):
        native.write_macos_native_record(
            role="core",
            wheel_path=wheel,
            signing_facts_path=_facts_file(tmp_path, _facts(b"root")),
            output_path=tmp_path / "bad.json",
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=CORE_LOCK,
        )
    assert not (tmp_path / ".bad.json.tmp").exists()


@pytest.mark.release
def test_signing_helper_removes_arbitrary_identity_override() -> None:
    source = Path("scripts/sign-and-notarize-helper.sh").read_text(encoding="utf-8")

    assert "CODESIGN_IDENTITY" not in source
    assert "MACOS_SIGNER_IDENTITY" in source


@pytest.mark.release
def test_signing_helper_records_tool_observations_not_pin_constants() -> None:
    source = Path("scripts/sign-and-notarize-helper.sh").read_text(encoding="utf-8")

    assert 'UNSIGNED_BINARY_SHA256="$(shasum -a 256 "$BINARY"' in source
    assert source.index("UNSIGNED_BINARY_SHA256=") < source.index(
        'echo "==> codesigning $BINARY'
    )
    assert '"unsigned_binary_sha256": os.environ["UNSIGNED_BINARY_SHA256"]' in source
    assert 'SWIFT_FIRST_LINE="$SWIFT_FIRST_LINE"' in source
    assert '"swift": os.environ["SWIFT_FIRST_LINE"]' in source
    assert 'NOTARYTOOL_OUTPUT="$NOTARYTOOL_OUTPUT"' in source
    assert '"notarytool": os.environ["NOTARYTOOL_OUTPUT"]' in source
    assert '"swift": os.environ["SWIFT_PIN"]' not in source
    assert '"notarytool": os.environ["NOTARYTOOL_PIN"]' not in source
