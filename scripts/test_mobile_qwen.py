#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Qwen resource assembly and image-admission controls without native loading."""

import hashlib
import copy
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


class BindingTests(unittest.TestCase):
    def record(self, platform):
        mobile = qwen.mobile_native_build
        pins = json.loads((qwen.ROOT / 'core/distribution/mobile/inputs.json').read_text())['inputs']
        names = ['core/distribution/mobile/llama-embedded-lifecycle.patch']
        if platform == 'android':
            names.append('core/distribution/mobile/llama-android-dotprod-repack.patch')
        else:
            names.append('core/distribution/mobile/llama-metal-ios-residency.patch')
        patches = [{'path': name, 'sha256': mobile.sha256(qwen.ROOT / name)} for name in names]
        inputs = {'driver_sha256': mobile.sha256(qwen.ROOT / 'scripts/mobile_native_build.py'),
                  'inputs_sha256': mobile.sha256(qwen.ROOT / 'core/distribution/mobile/inputs.json'),
                  'support_driver_sha256': mobile.sha256(qwen.ROOT / 'scripts/android_cross_build.py'),
                  'openssl': pins['openssl']}
        binding, identity = mobile.qwen_runtime_binding(
            platform, pins['llama'], '1' * 64, patches, qwen.ROOT / 'tools/mobile-qwen-engine', inputs)
        return {'schema': 'solstone.mobile-qwen-package.v1', 'binding': binding,
                'runtime_identity': identity, 'image': qwen.IMAGES[platform], 'signed': False,
                'exports': sorted(mobile.QWEN_EXPORTS)}

    def test_current_android_patch_is_required_with_unchanged_four_worker_policy(self):
        record = self.record('android')
        qwen.validate_record(record, 'android')
        self.assertEqual(record['binding']['policy']['cpu_workers'], 4)
        self.assertEqual((record['binding']['policy']['batch_tokens'],
                          record['binding']['policy']['ubatch_tokens']), (2048, 512))
        # Actual pre-alias Android patch digest in the retained FAST/A4 SDKs.
        old = copy.deepcopy(record)
        old['binding']['patches'][-1]['sha256'] = '28f41789f0d263883ec726ab4aa72f71cbb510a6f27dd3bc2a31d3ae9df73f97'
        old['runtime_identity'] = 'sha256:' + hashlib.sha256(json.dumps(
            old['binding'], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, 'maintained source'):
            qwen.validate_record(old, 'android')

    def test_ios_requires_encoder_residency_and_current_patch_without_diagnostics(self):
        record = self.record('ios')
        qwen.validate_record(record, 'ios')
        self.assertEqual(record['binding']['policy']['metal_resource_residency'], 'encoder-managed')
        self.assertEqual(record['binding']['policy']['context_tokens'], 32768)
        self.assertEqual(record['binding']['policy']['parallel_slots'], 1)
        self.assertEqual(record['binding']['policy']['cpu_workers'], 4)
        for variant in ('missing-policy', 'wrong-policy', 'wrong-type', 'missing-patch',
                        'wrong-patch', 'legacy', 'diagnostic', 'private-diagnostic'):
            with self.subTest(variant=variant):
                changed = copy.deepcopy(record)
                binding = changed['binding']
                if variant in ('missing-policy', 'legacy'):
                    binding['policy'].pop('metal_resource_residency')
                elif variant == 'wrong-policy':
                    binding['policy']['metal_resource_residency'] = 'explicit-sets'
                elif variant == 'wrong-type':
                    binding['policy']['metal_resource_residency'] = False
                if variant in ('missing-patch', 'legacy'):
                    binding['patches'].pop()
                elif variant == 'wrong-patch':
                    binding['patches'][-1]['sha256'] = '0' * 64
                elif variant in ('diagnostic', 'private-diagnostic'):
                    binding['diagnostics'] = {'metal_no_residency': True}
                    if variant == 'private-diagnostic':
                        binding['policy'].pop('metal_resource_residency')
                        binding['patches'][-1]['path'] = 'core/distribution/mobile/llama-metal-residency-diagnostic.patch'
                # A self-consistent digest must not admit a different policy.
                changed['runtime_identity'] = 'sha256:' + hashlib.sha256(json.dumps(
                    binding, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
                with self.assertRaisesRegex(ValueError, 'maintained source'):
                    qwen.validate_record(changed, 'ios')
        changed = copy.deepcopy(record)
        changed['runtime_identity'] = 'sha256:' + '0' * 64
        with self.assertRaisesRegex(ValueError, 'maintained source'):
            qwen.validate_record(changed, 'ios')

    def test_android_keeps_its_policy_and_does_not_consume_ios_patch(self):
        record = self.record('android')
        self.assertNotIn('metal_resource_residency', record['binding']['policy'])
        real_sha = qwen.mobile_native_build.sha256

        def sha(path):
            if Path(path).name == 'llama-metal-ios-residency.patch':
                raise AssertionError('iOS-only patch read by Android admission')
            return real_sha(path)

        with mock.patch.object(qwen.mobile_native_build, 'sha256', side_effect=sha):
            qwen.validate_record(record, 'android')
        record['binding']['policy']['metal_resource_residency'] = 'encoder-managed'
        record['runtime_identity'] = 'sha256:' + hashlib.sha256(json.dumps(
            record['binding'], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, 'maintained source'):
            qwen.validate_record(record, 'android')

    def test_ios_validation_does_not_consume_android_patch(self):
        record = self.record('ios')
        real_sha = qwen.mobile_native_build.sha256

        def sha(path):
            if Path(path).name == 'llama-android-dotprod-repack.patch':
                raise AssertionError('Android-only patch read by iOS admission')
            return real_sha(path)

        with mock.patch.object(qwen.mobile_native_build, 'sha256', side_effect=sha):
            qwen.validate_record(record, 'ios')


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
