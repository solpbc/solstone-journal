#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Pure package-admission checks; native loading is verified on each target."""

from contextlib import ExitStack
import hashlib
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import package_mobile_pdfium as pdfium


def elf(*, machine=183, alignment=16384, dependency=b"libc.so"):
    data = bytearray(1024)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HH", data, 16, 3, machine)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HH", data, 54, 56, 2)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0, 0, len(data), len(data), alignment)
    struct.pack_into("<IIQQQQQQ", data, 120, 2, 6, 256, 256, 256, 64, 64, 8)
    strings = b"\0" + dependency + b"\0"
    data[512:512 + len(strings)] = strings
    for at, pair in enumerate([(1, 1), (5, 512), (10, len(strings)), (0, 0)]):
        struct.pack_into("<qQ", data, 256 + 16 * at, *pair)
    return bytes(data)


def macho(*, platform=2, minimum=26 << 16, kind=6, with_version=True,
          dependency="/usr/lib/libSystem.B.dylib"):
    commands = []
    if with_version:
        commands.append(struct.pack("<IIIIII", 0x32, 24, platform, minimum, 26 << 16, 0))
    for command, name in [(0xD, pdfium.INSTALL_ID), (0xC, dependency)]:
        value = name.encode() + b"\0"
        size = (24 + len(value) + 7) // 8 * 8
        commands.append(struct.pack("<IIIIII", command, size, 24, 0, 0, 0)
                        + value + bytes(size - 24 - len(value)))
    payload = b"".join(commands)
    return struct.pack("<IIIIIIII", 0xFEEDFACF, 0x100000C, 0, kind, len(commands),
                       len(payload), 0, 0) + payload


def archive_at(path, entries):
    with tarfile.open(path, "w") as bundle:
        for name, data in entries:
            row = tarfile.TarInfo(name)
            row.size = len(data)
            bundle.addfile(row, io.BytesIO(data))


class InspectionTests(unittest.TestCase):
    def test_android_native_contract(self):
        self.assertEqual(pdfium.inspect_android(elf())["dependencies"], ["libc.so"])
        for data in [elf(machine=62), elf(alignment=4096), elf(dependency=b"libprivate.so"),
                     elf()[:600], b"not an elf"]:
            with self.subTest(data=data[:20]), self.assertRaises(ValueError):
                pdfium.inspect_android(data)

    def test_ios_requires_device_version_and_dynamic_library(self):
        self.assertEqual(pdfium.inspect_ios(macho(), pdfium.INSTALL_ID)["minimum_os"], [26, 0, 0])
        for kwargs in [{"platform": 7}, {"minimum": 0}, {"minimum": 27 << 16},
                       {"kind": 2}, {"with_version": False},
                       {"dependency": "@rpath/Unpackaged.framework/Unpackaged"}]:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                pdfium.inspect_ios(macho(**kwargs), pdfium.INSTALL_ID)

    def test_archive_rejects_unsafe_notice_and_missing_license(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "input.tar"
            for entries in [[("lib/libpdfium.so", elf())],
                            [("lib/libpdfium.so", elf()), ("LICENSE", b"license"),
                             ("licenses/../outside", b"bad")]]:
                archive_at(archive, entries)
                with self.assertRaises(ValueError):
                    pdfium.archive_members(archive, "android")


class AssemblyTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        archive = self.root / "input.tar"
        archive_at(archive, [("lib/libpdfium.so", elf()), ("LICENSE", b"upstream license"),
                             ("licenses/pdfium.txt", b"pdfium notice"),
                             ("licenses/other.txt", b"dependency notice")])
        self.pin = {"sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}
        self.stack.enter_context(mock.patch.object(pdfium, "pin_for", return_value=self.pin))
        self.stack.enter_context(mock.patch.object(pdfium, "sources", return_value={"fixture": "source"}))
        self.stack.enter_context(mock.patch.object(pdfium, "acquire", return_value=archive))
        self.output = self.root / "staged"
        self.record = pdfium.stage("android", self.root / "cache", self.output)
        self.runtime = self.root / "runtime"
        native = self.runtime / "lib/arm64-v8a/libpdfium.so"
        native.parent.mkdir(parents=True)
        native.write_bytes(elf())

    def assembled(self, *, corrupt=None, omit=None):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            members = [(self.output / "libpdfium.so", "lib/arm64-v8a/libpdfium.so"),
                       *pdfium.android_members(self.output, self.runtime)]
            for source, name in members:
                if name != omit:
                    archive.writestr(name, b"changed" if name == corrupt else source.read_bytes())
        stream.seek(0)
        return zipfile.ZipFile(stream)

    def test_final_consumer_binds_every_library_and_notice(self):
        with self.assembled() as archive:
            pdfium.verify_android_archive(self.output, archive)
        for name in ["lib/arm64-v8a/libpdfium.so", "assets/pdfium-notices/LICENSE",
                     "assets/pdfium-notices/licenses/other.txt"]:
            for operation in ["corrupt", "omit"]:
                with self.subTest(name=name, operation=operation):
                    with self.assembled(**{operation: name}) as archive, self.assertRaises(ValueError):
                        pdfium.verify_android_archive(self.output, archive)

    def test_changed_staging_is_not_reblessed(self):
        for relative in ["libpdfium.so", "notices/LICENSE", "notices/licenses/other.txt"]:
            path = self.output / relative
            original = path.read_bytes()
            path.write_bytes(b"changed")
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                pdfium.stage("android", self.root / "cache", self.output)
            path.write_bytes(original)

    def test_changed_runtime_library_or_receipt_path_refuses(self):
        native = self.runtime / "lib/arm64-v8a/libpdfium.so"
        native.write_bytes(b"wrong")
        with self.assertRaises(ValueError):
            pdfium.android_members(self.output, self.runtime)
        record = json.loads((self.output / "receipt.json").read_text())
        record["members"][0]["path"] = "../outside"
        (self.output / "receipt.json").write_text(json.dumps(record))
        with self.assertRaises(ValueError):
            pdfium.validate_staged(self.output, "android")

    def test_symlinked_notice_directory_refuses(self):
        notices = self.output / "notices"
        notices.rename(self.output / "other")
        notices.symlink_to(self.output / "other", target_is_directory=True)
        with self.assertRaises(ValueError):
            pdfium.validate_staged(self.output, "android")


class IOSEmbeddingTests(unittest.TestCase):
    def test_parent_alias_allowed_but_app_framework_symlink_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            stage = root / "stage"
            stage.mkdir()
            library = stage / "PDFium.framework/PDFium"
            library.parent.mkdir()
            library.write_bytes(macho())
            record = {"members": [pdfium.member(library, "PDFium.framework/PDFium")]}
            real = root / "real"
            app = real / "Test.app"
            app.mkdir(parents=True)
            alias = root / "alias"
            alias.symlink_to(real, target_is_directory=True)
            with mock.patch.object(pdfium, "validate_staged", return_value=record):
                pdfium.embed_ios(stage, alias / "Test.app")
                self.assertEqual((app / "Frameworks/PDFium.framework/PDFium").read_bytes(), macho())
                (app / "Frameworks").rename(app / "OldFrameworks")
                (app / "Frameworks").symlink_to(app / "OldFrameworks", target_is_directory=True)
                with self.assertRaises(ValueError):
                    pdfium.embed_ios(stage, app)


if __name__ == "__main__":
    unittest.main()
