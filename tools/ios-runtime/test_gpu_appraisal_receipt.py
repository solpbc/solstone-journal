#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import pathlib
import struct
import tempfile
import tomllib
import unittest
from types import SimpleNamespace

from build_gpu_appraisal import (
    _configure_argv,
    _prepare_host_build_profile,
    _record_digest,
    _static_dependencies,
)
from finalize import (
    ROOT,
    canonical_digest,
    macho_content_sha256,
    package_members,
    prepare_rim_directory,
    sha256,
    validate_build_inputs,
)


class AppraisalBuildTests(unittest.TestCase):
    def test_generated_host_profile_records_scope_and_preserves_sdk_source(self):
        with tempfile.TemporaryDirectory() as temp:
            work = pathlib.Path(temp)
            sdk_manifest = work / "sources/regorus/Cargo.toml"
            sdk_manifest.parent.mkdir(parents=True)
            original = b'[profile.release]\nstrip = true\n'
            sdk_manifest.write_bytes(original)
            build = SimpleNamespace(work=work, summary={"inputs": {}}, save=lambda: None)
            _prepare_host_build_profile(build)
            configuration = work / ".cargo/config.toml"
            self.assertEqual(tomllib.loads(configuration.read_text()), {
                "profile": {"release": {"build-override": {"strip": "none"}}}})
            recorded = build.summary["inputs"]["appraisal-host-build-profile"]
            self.assertEqual(recorded["sha256"], sha256(configuration))
            self.assertEqual(work / recorded["path"], configuration)
            self.assertEqual(sdk_manifest.read_bytes(), original)

    def test_rim_directory_reuse_preserves_existing_members(self):
        with tempfile.TemporaryDirectory() as temp:
            rims = pathlib.Path(temp) / "RIMs"
            prepare_rim_directory(rims, {"driver.xml"})
            member = rims / "driver.xml"
            member.write_bytes(b"existing")
            prepare_rim_directory(rims, {"driver.xml"})
            self.assertEqual(member.read_bytes(), b"existing")

    def test_rim_directory_reuse_refuses_symlinks_and_directories(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            target = root / "outside"
            target.mkdir()
            rims = root / "RIMs"
            rims.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink"):
                prepare_rim_directory(rims, {"driver.xml"})
            rims.unlink()
            rims.mkdir()
            (rims / "driver.xml").symlink_to(target / "outside.xml")
            with self.assertRaisesRegex(ValueError, "non-file"):
                prepare_rim_directory(rims, {"driver.xml"})
            (rims / "driver.xml").unlink()
            (rims / "unexpected").mkdir()
            with self.assertRaisesRegex(ValueError, "non-file"):
                prepare_rim_directory(rims, {"driver.xml"})
            (rims / "unexpected").rmdir()
            (rims / "unexpected.xml").write_bytes(b"unknown")
            with self.assertRaisesRegex(ValueError, "unexpected member"):
                prepare_rim_directory(rims, {"driver.xml"})

    def test_static_reconfigure_preserves_exact_driver_command(self):
        build = SimpleNamespace(
            summary={
                "steps": [
                    {
                        "name": "nvattest-configure",
                        "argv": [
                            "cmake",
                            "-S",
                            "/source",
                            "-B",
                            "/build",
                            "-DBUILD_SHARED_LIBS=ON",
                            "-DFETCHCONTENT_SOURCE_DIR_REGORUS=/pinned",
                        ],
                    }
                ]
            }
        )
        self.assertEqual(
            _configure_argv(build),
            [
                "cmake",
                "-S",
                "/source",
                "-B",
                "/build",
                "-DFETCHCONTENT_SOURCE_DIR_REGORUS=/pinned",
                "-DBUILD_SHARED_LIBS=OFF",
                "-DCMAKE_SKIP_INSTALL_RULES=ON",
            ],
        )

    def test_dependency_census_is_ordered_and_deduplicated(self):
        root = pathlib.Path("/tmp/build")
        command = "clang -o libnvat.dylib dep/a.a /fixed/b.a dep/a.a -lz"
        self.assertEqual(
            _static_dependencies(command, root),
            [(root / "dep/a.a").resolve(), pathlib.Path("/fixed/b.a")],
        )

    def test_build_record_digest_excludes_no_fields_implicitly(self):
        record = {"complete": True, "steps": [{"exit": 0}]}
        self.assertEqual(_record_digest(record), canonical_digest(record))
        self.assertNotEqual(
            _record_digest(record),
            _record_digest({**record, "extra": "substitution"}),
        )

    def test_package_census_binds_all_non_signature_members(self):
        with tempfile.TemporaryDirectory(dir="/var/tmp") as temp:
            root = pathlib.Path(temp)
            (root / "_CodeSignature").mkdir()
            (root / "_CodeSignature/CodeResources").write_bytes(b"mutable signature")
            (root / "gpu-appraisal-receipt.json").write_text("{}")
            header = bytearray(32)
            header[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<II", header, 16, 1, 16)
            command = struct.pack("<IIII", 0x1D, 16, 48, 4)
            (root / "Appraisal").write_bytes(header + command + b"sign")
            (root / "Info.plist").write_bytes(b"plist")
            members = package_members(root)
            self.assertEqual([item["path"] for item in members], ["Appraisal", "Info.plist"])

    def test_macho_content_digest_excludes_only_signature_blob(self):
        with tempfile.TemporaryDirectory(dir="/var/tmp") as temp:
            path = pathlib.Path(temp) / "Appraisal"
            header = bytearray(32)
            header[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<II", header, 16, 1, 16)
            command = struct.pack("<IIII", 0x1D, 16, 48, 4)
            path.write_bytes(header + command + b"aaaa")
            first = macho_content_sha256(path)
            path.write_bytes(header + command + b"bbbb")
            self.assertEqual(first, macho_content_sha256(path))
            header[8] = 1
            path.write_bytes(header + command + b"bbbb")
            self.assertNotEqual(first, macho_content_sha256(path))

    def test_package_census_refuses_symlinks(self):
        with tempfile.TemporaryDirectory(dir="/var/tmp") as temp:
            root = pathlib.Path(temp)
            (root / "target").write_bytes(b"x")
            (root / "alias").symlink_to("target")
            with self.assertRaisesRegex(ValueError, "symlink"):
                package_members(root)

    def test_build_record_inputs_must_match_source_tree(self):
        record = {
            "mobile_native_build": {
                "driver_sha256": sha256(ROOT / "scripts/mobile_native_build.py"),
                "inputs_sha256": sha256(
                    ROOT / "core/distribution/mobile/inputs.json"
                ),
                "patches": [
                    {
                        "component": "nvattest",
                        "sha256": sha256(
                            ROOT
                            / "core/distribution/mobile/nvattest-mobile.patch"
                        ),
                    }
                ],
            }
        }
        validate_build_inputs(record)
        record["mobile_native_build"]["inputs_sha256"] = "00" * 32
        with self.assertRaisesRegex(ValueError, "inputs_sha256"):
            validate_build_inputs(record)

    def test_build_record_requires_one_matching_nvattest_patch(self):
        record = {
            "mobile_native_build": {
                "driver_sha256": sha256(ROOT / "scripts/mobile_native_build.py"),
                "inputs_sha256": sha256(
                    ROOT / "core/distribution/mobile/inputs.json"
                ),
                "patches": [],
            }
        }
        with self.assertRaisesRegex(ValueError, "nvattest patch"):
            validate_build_inputs(record)


if __name__ == "__main__":
    unittest.main()
