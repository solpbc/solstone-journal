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
