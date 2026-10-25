# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Bind owner APK speech inputs to the existing runtime authorities."""
import hashlib
import json
import pathlib
import re


def audio_package(repo, model, catalog):
    repo = pathlib.Path(repo)
    rows = [row for row in json.loads(pathlib.Path(catalog).read_text()) if row['unit'] == 'parakeet-model']
    if len(rows) != 1:
        raise ValueError('one current Parakeet model pin required')
    pin = rows[0]
    sources = [(pathlib.Path(model), pin['filename'], pin['sha256'], pin['size_bytes'], 'models')]
    constants = (repo / 'core/crates/solstone-core-transcribe/src/model_assets.rs').read_text()
    for name, constant in [('firered_vad.onnx', 'FIRERED_VAD_SHA256'),
                           ('wespeaker-resnet34-256.onnx', 'WESPEAKER_RESNET34_SHA256'),
                           ('pyannote-segmentation-3.0.onnx', 'PYANNOTE_SEGMENTATION_SHA256')]:
        hashes = re.findall(r'pub const ' + constant + r': &str =\s*"([0-9a-f]{64})";', constants)
        if len(hashes) != 1:
            raise ValueError('one consuming runtime pin required: ' + constant)
        sources.append((repo / 'core/models/assets' / name, name, hashes[0], None, 'models'))
    for source, name in [('core/distribution/licenses/fireredvad/LICENSE', 'FireRedVAD-LICENSE.txt'),
                         ('core/distribution/model-notices.md', 'NOTICE.md')]:
        sources.append((repo / source, name, None, None, 'notices'))
    manifest = {'models': [], 'notices': []}
    members = []
    for source, name, expected, size, group in sources:
        if source.is_symlink() or not source.is_file() or pathlib.PurePath(name).name != name:
            raise ValueError('regular canonical speech input required: ' + name)
        with source.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        length = source.stat().st_size
        if expected is not None and digest != expected or size is not None and length != size:
            raise ValueError('speech input differs from consuming runtime pin: ' + name)
        if group == 'notices':
            # The additive installer preserves old copies across APK updates.
            notice = pathlib.PurePath(name)
            name = notice.stem + '-' + digest + notice.suffix
        manifest[group].append({'name': name, 'bytes': length, 'sha256': digest})
        members.append((source, 'assets/audio-models/' + ('notices/' if group == 'notices' else '') + name))
    return manifest, members
