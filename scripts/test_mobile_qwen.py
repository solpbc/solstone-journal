#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Qwen resource assembly and image-admission controls without native loading."""

import hashlib
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock
import zipfile

import package_mobile_qwen as qwen
from test_mobile_pdfium import elf


def qwen_elf(*, relro_size=16384):
    data = bytearray(elf())
    struct.pack_into("<HH", data, 54, 56, 3)
    struct.pack_into("<IIQQQQQQ", data, 176, 0x6474E552, 4, 0, 0, 0,
                     0, relro_size, 1)
    return bytes(data)


class BindingTests(unittest.TestCase):
    def record(self, diagnostic):
        mobile = qwen.mobile_native_build
        pins = json.loads((qwen.ROOT / "core/distribution/mobile/inputs.json").read_text())["inputs"]
        names = ["core/distribution/mobile/llama-embedded-lifecycle.patch"]
        if diagnostic:
            names.append("core/distribution/mobile/llama-metal-residency-diagnostic.patch")
        patches = [{"path": name, "sha256": mobile.sha256(qwen.ROOT / name)} for name in names]
        inputs = {"driver_sha256": mobile.sha256(qwen.ROOT / "scripts/mobile_native_build.py"),
                  "inputs_sha256": mobile.sha256(qwen.ROOT / "core/distribution/mobile/inputs.json"),
                  "support_driver_sha256": mobile.sha256(qwen.ROOT / "scripts/android_cross_build.py"),
                  "openssl": pins["openssl"]}
        binding, identity = mobile.qwen_runtime_binding(
            "ios", pins["llama"], "4" * 64, patches, qwen.ROOT / "tools/mobile-qwen-engine",
            inputs, metal_no_residency=diagnostic)
        return {"schema": "solstone.mobile-qwen-package.v1", "signed": False,
                "image": qwen.IMAGES["ios"], "binding": binding,
                "runtime_identity": identity, "exports": sorted(mobile.QWEN_EXPORTS)}

    def test_admission_requires_the_diagnostic_marker_patch_and_identity_to_agree(self):
        for diagnostic in (False, True):
            qwen.validate_record(self.record(diagnostic), "ios")
        record = self.record(True)
        record["binding"].pop("diagnostics")
        with self.assertRaisesRegex(ValueError, "maintained source"):
            qwen.validate_record(record, "ios")
        record = self.record(True)
        record["binding"]["patches"] = self.record(False)["binding"]["patches"]
        with self.assertRaisesRegex(ValueError, "maintained source"):
            qwen.validate_record(record, "ios")
        record = self.record(True)
        record["runtime_identity"] = self.record(False)["runtime_identity"]
        with self.assertRaisesRegex(ValueError, "maintained source"):
            qwen.validate_record(record, "ios")

    def test_admission_refuses_unknown_non_boolean_or_android_diagnostics(self):
        for diagnostics in ({"metal_no_residency": 1}, {"metal_no_residency": False},
                            {"other": True}):
            record = self.record(True)
            record["binding"]["diagnostics"] = diagnostics
            with self.assertRaisesRegex(ValueError, "unsupported.*diagnostic"):
                qwen.validate_record(record, "ios")
        with self.assertRaisesRegex(ValueError, "unsupported.*diagnostic"):
            qwen.validate_record(self.record(True), "android")


class ResourceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.package = self.root / "package"
        self.package.mkdir()
        for name in qwen.PACKAGE_LICENSES:
            (self.package / name).write_bytes(("producer " + name).encode())
        self.image = self.package / qwen.IMAGES["android"]
        self.image.write_bytes(qwen_elf())
        self.write_record()
        patch = mock.patch.object(qwen, "validate_record")
        patch.start()
        self.addCleanup(patch.stop)

    def write_record(self):
        data = self.image.read_bytes()
        record = {"image_sha256": hashlib.sha256(data).hexdigest(),
                  "image_size_bytes": len(data)}
        (self.package / "manifest.json").write_text(json.dumps(record))

    def assembled(self, runtime, *, corrupt=None):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            for source in sorted(runtime.rglob("*")):
                if source.is_file():
                    name = source.relative_to(runtime).as_posix()
                    archive.writestr(name, b"changed" if name == corrupt else source.read_bytes())
        stream.seek(0)
        return zipfile.ZipFile(stream)

    def test_android_carries_source_notice_and_producer_licenses(self):
        runtime = self.root / "runtime"
        qwen.stage_android(self.package, runtime)
        assets = runtime / "assets/qwen-runtime"
        self.assertFalse((self.package / qwen.THIRD_PARTY_NOTICES).exists())
        qwen.verify_licenses(self.package, assets)
        with self.assembled(runtime) as archive:
            qwen.verify_android_archive(archive, runtime)
            for name in qwen.LICENSES:
                self.assertEqual(archive.read("assets/qwen-runtime/" + name),
                                 qwen.license_bytes(self.package, name))
        for name in qwen.LICENSES:
            with self.subTest(name=name), self.assembled(
                    runtime, corrupt="assets/qwen-runtime/" + name) as archive:
                with self.assertRaises(ValueError):
                    qwen.verify_android_archive(archive, runtime)

    def test_changed_staging_and_archive_cannot_rebless_source_notice(self):
        runtime = self.root / "runtime"
        qwen.stage_android(self.package, runtime)
        notice = runtime / "assets/qwen-runtime" / qwen.THIRD_PARTY_NOTICES
        notice.write_bytes(b"changed")
        with self.assembled(runtime) as archive, self.assertRaises(ValueError):
            qwen.verify_android_archive(archive, runtime)

    def test_ios_resource_helper_rejects_changed_or_symlinked_notice(self):
        resources = self.root / "Fixture.app/QwenRuntime"
        resources.mkdir(parents=True)
        qwen.copy_licenses(self.package, resources)
        qwen.verify_licenses(self.package, resources)
        notice = resources / qwen.THIRD_PARTY_NOTICES
        notice.write_bytes(b"changed")
        with self.assertRaises(ValueError):
            qwen.verify_licenses(self.package, resources)
        notice.unlink()
        notice.symlink_to(qwen.ROOT / "core/distribution/mobile" / qwen.THIRD_PARTY_NOTICES)
        with self.assertRaises(ValueError):
            qwen.verify_licenses(self.package, resources)
        with self.assertRaises(ValueError):
            qwen.copy_licenses(self.package, resources)

    def test_native_image_hash_and_geometry_checks_remain_required(self):
        self.image.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "native receipt"):
            qwen.load_package(self.package, "android")
        self.image.write_bytes(qwen_elf(relro_size=4096))
        self.write_record()
        with self.assertRaisesRegex(ValueError, "16 KiB"):
            qwen.load_package(self.package, "android")


if __name__ == "__main__":
    unittest.main()
