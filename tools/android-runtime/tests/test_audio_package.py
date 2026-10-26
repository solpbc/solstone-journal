# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from audio_package import audio_package, verify_audio_archive
from package_mobile_media import SOUND_TAG_NOTICES


class PackageUpgrade(unittest.TestCase):
    def test_changed_notice_keeps_models_and_prior_notice(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model = root / 'model.gguf'
            model.write_bytes(b'unchanged speech model')
            assets = root / 'core/models/assets'
            assets.mkdir(parents=True)
            constants = root / 'core/crates/solstone-core-transcribe/src/model_assets.rs'
            constants.parent.mkdir(parents=True)
            text = []
            for name, constant in [('firered_vad.onnx', 'FIRERED_VAD_SHA256'),
                                   ('wespeaker-resnet34-256.onnx', 'WESPEAKER_RESNET34_SHA256'),
                                   ('pyannote-segmentation-3.0.onnx', 'PYANNOTE_SEGMENTATION_SHA256')]:
                payload = name.encode()
                (assets / name).write_bytes(payload)
                text.append('pub const ' + constant + ': &str = "' + hashlib.sha256(payload).hexdigest() + '";')
            constants.write_text('\n'.join(text))
            catalog = root / 'catalog.json'
            catalog.write_text(json.dumps([dict(unit='parakeet-model', filename=model.name,
                sha256=hashlib.sha256(model.read_bytes()).hexdigest(), size_bytes=model.stat().st_size)]))
            ced = assets / 'ced/ced.gguf'
            ced.parent.mkdir()
            ced.write_bytes(b'unchanged CED model')
            authority = root / 'core/crates/solstone-core-assets/src/lib.rs'
            authority.parent.mkdir(parents=True)
            authority.write_text('Artifact {\nunit: "ced-model",\nfilename: "ced.gguf",\nsha256: "' +
                                 hashlib.sha256(ced.read_bytes()).hexdigest() + '",\nsize_bytes: ' +
                                 str(ced.stat().st_size) + '\n    },')
            license = root / 'core/distribution/licenses/fireredvad/LICENSE'
            license.parent.mkdir(parents=True)
            license.write_bytes(b'unchanged license')
            notice = root / 'core/distribution/model-notices.md'
            notice.write_bytes(b'old aggregate notice')
            for relative, _ in SOUND_TAG_NOTICES:
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(relative.encode())
            before, old_members = audio_package(root, model, catalog)
            old_notice = next(pin for pin in before['notices'] if pin['name'].startswith('NOTICE-'))
            notice.write_bytes(b'new aggregate notice')
            after, members = audio_package(root, model, catalog)
            new_notice = next(pin for pin in after['notices'] if pin['name'].startswith('NOTICE-'))
            self.assertNotEqual(old_notice['name'], new_notice['name'])
            self.assertEqual(before['models'], after['models'])
            self.assertEqual(before['notices'][0], after['notices'][0])
            paths = dict((target, source) for source, target in members)
            self.assertEqual(paths['assets/audio-models/notices/' + new_notice['name']].read_bytes(), b'new aggregate notice')
            original_ced = ced.read_bytes()
            ced_name = 'assets/audio-models/' + ced.name
            for mode in ['valid', 'changed', 'duplicate']:
                archive = root / (mode + '.apk')
                if mode == 'changed':
                    ced.write_bytes(b'x' * len(original_ced))
                with zipfile.ZipFile(archive, 'w') as output:
                    output.writestr('assets/audio-models/manifest.json', json.dumps(after))
                    for source, name in members:
                        output.write(source, name)
                    if mode == 'duplicate':
                        output.writestr(ced_name, original_ced)
                ced.write_bytes(original_ced)
                with zipfile.ZipFile(archive) as packaged:
                    if mode == 'valid':
                        verify_audio_archive(packaged, after)
                    else:
                        with self.assertRaises(ValueError): verify_audio_archive(packaged, after)
            model.write_bytes(b'conflicting model bytes')
            with self.assertRaises(ValueError):
                audio_package(root, model, catalog)


if __name__ == '__main__':
    unittest.main()
