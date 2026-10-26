#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

import package_mobile_media as media


class MediaPackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.models = self.repo / 'models'
        self.models.mkdir()
        self.detector = self.models / 'model.gguf'
        self.detector.write_bytes(b'pinned object model')
        rfdetr = self.repo / 'tools/mobile-serving/src/rfdetr.rs'
        rfdetr.parent.mkdir(parents=True)
        rfdetr.write_text(self.constants('MODEL', self.detector.read_bytes()) + '\nconst PACKAGED_MODEL: &str = "lib/solstone-rfdetr/models/rfdetr-nano-f16.gguf";\n')
        ocr = ['const MODEL_SUBDIR: &str = "lib/solstone-screen-ocr/models";']
        recipe = []
        for key, name in [('detector', 'detector.onnx'), ('recognizer', 'recognizer.onnx'), ('dictionary', 'dictionary.txt')]:
            data = ('pinned ' + key).encode()
            (self.models / name).write_bytes(data)
            ocr.append(self.constants(key.upper(), data))
            ocr.append(f'const {key.upper()}_NAME: &str = "{name}";')
            digest_key = 'materialized_sha256' if key == 'dictionary' else 'sha256'
            recipe.append(f'[models.{key}]\nfilename = "{name}"\n{digest_key} = "{hashlib.sha256(data).hexdigest()}"\n')
        (rfdetr.parent / 'screen_ocr.rs').write_text('\n'.join(ocr))
        helper = self.repo / 'core/screen-ocr-helper'
        helper.mkdir(parents=True)
        (helper / 'recipe.toml').write_text('\n'.join(recipe))
        for relative in ['core/screen-ocr-helper/NOTICE', 'core/screen-ocr-helper/LICENSE-APACHE', 'core/distribution/model-notices.md', 'core/distribution/licenses/rfdetr/LICENSE']:
            target = self.repo / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text('retained notice')

    @staticmethod
    def constants(prefix, data):
        return f'const {prefix}_SHA256: &str = "{hashlib.sha256(data).hexdigest()}";\nconst {prefix}_SIZE: u64 = {len(data)};'

    def package(self):
        return media.media_package(self.repo, self.detector, self.models)

    def test_both_layouts_contain_exact_admitted_models_and_notices(self):
        manifest, members = self.package()
        destination = self.repo / 'payload'
        media.stage(destination, manifest, members)
        self.assertEqual(media.verify_payload(self.repo, destination), manifest)
        self.assertEqual(json.loads((destination / 'media-models.json').read_text()), manifest)
        archive = self.repo / 'payload.zip'
        prefix = 'assets/journal-payload/'
        with zipfile.ZipFile(archive, 'w') as bundle:
            bundle.writestr(prefix + 'media-models.json', json.dumps(manifest))
            for source, name in members:
                bundle.write(source, prefix + name)
        with zipfile.ZipFile(archive) as bundle:
            media.verify_archive(bundle, manifest, prefix)
        for row in manifest['files']:
            self.assertEqual(hashlib.sha256((destination / row['name']).read_bytes()).hexdigest(), row['sha256'])

    def test_changed_or_missing_model_refuses(self):
        self.detector.write_bytes(b'changed object model')
        with self.assertRaises(ValueError): self.package()
        self.detector.unlink()
        with self.assertRaises(ValueError): self.package()

    def test_recipe_and_runtime_disagreement_refuses(self):
        target = self.repo / 'tools/mobile-serving/src/screen_ocr.rs'
        target.write_text(target.read_text().replace('detector.onnx', 'other.onnx'))
        with self.assertRaises(ValueError): self.package()

    def test_post_admission_change_and_duplicate_archive_member_refuse(self):
        manifest, members = self.package()
        original = self.detector.read_bytes()
        self.detector.write_bytes(b'changed after admission')
        with self.assertRaises(ValueError): media.stage(self.repo / 'payload', manifest, members)
        self.detector.write_bytes(original)
        archive = self.repo / 'duplicate.zip'
        with zipfile.ZipFile(archive, 'w') as bundle:
            bundle.writestr('media-models.json', json.dumps(manifest))
            for source, name in members: bundle.write(source, name)
            bundle.writestr(members[0][1], b'duplicate')
        with zipfile.ZipFile(archive) as bundle:
            with self.assertRaises(ValueError): media.verify_archive(bundle, manifest, '')

    def test_final_payload_rechecks_consuming_pins_and_copied_bytes(self):
        manifest, members = self.package()
        destination = self.repo / 'payload'
        media.stage(destination, manifest, members)
        model = destination / members[0][1]
        original = model.read_bytes()
        model.write_bytes(b'corrupt copied model')
        with self.assertRaises(ValueError): media.verify_payload(self.repo, destination)
        model.write_bytes(original)
        runtime = self.repo / 'tools/mobile-serving/src/rfdetr.rs'
        runtime.write_text(runtime.read_text().replace(hashlib.sha256(original).hexdigest(), '0' * 64))
        with self.assertRaises(ValueError): media.verify_payload(self.repo, destination)

    def test_sound_tag_model_requires_current_catalog_and_exact_copied_bytes(self):
        payload = b'maintained sound tag model'
        model = self.models / 'ced.gguf'
        model.write_bytes(payload)
        catalog = self.repo / 'core/crates/solstone-core-assets/src/lib.rs'
        catalog.parent.mkdir(parents=True)
        row = ('Artifact {\nunit: "ced-model",\nfilename: "ced.gguf",\nsha256: "' +
               hashlib.sha256(payload).hexdigest() + '",\nsize_bytes: ' + str(len(payload)) + '\n    },\n')
        catalog.write_text(row)
        expected = media.verify_sound_tag_model(self.repo, self.models)
        self.assertEqual(expected['filename'], 'ced.gguf')
        model.write_bytes(b'x' * len(payload))
        with self.assertRaises(ValueError): media.verify_sound_tag_model(self.repo, self.models)
        model.write_bytes(payload)
        catalog.write_text(row.replace('ced.gguf', '../ced.gguf'))
        with self.assertRaises(ValueError): media.verify_sound_tag_model(self.repo, self.models)
        catalog.write_text(row + row)
        with self.assertRaises(ValueError): media.verify_sound_tag_model(self.repo, self.models)


if __name__ == '__main__': unittest.main()
