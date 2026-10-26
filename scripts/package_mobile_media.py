#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Admit and stage the media models consumed by both embedded adapters."""
from pathlib import Path, PurePosixPath
import hashlib
import json
import re
import shutil
import tomllib

SOUND_TAG_NOTICES = (
    ('core/distribution/licenses/ced/LICENSE', 'CED-LICENSE.txt'),
    ('core/distribution/licenses/ced/ggml-LICENSE', 'CED-GGML-LICENSE.txt'),
    ('core/distribution/licenses/ced/ced-tiny-model-LICENSE', 'CED-model-LICENSE.txt'),
    ('core/distribution/licenses/ced/ATTRIBUTION.md', 'CED-ATTRIBUTION.md'),
)


def _constant(source, name, kind):
    if kind == 'text':
        pattern = r'const ' + name + r': &str =\s*"([^"]+)";'
    elif kind == 'sha256':
        pattern = r'const ' + name + r': &str =\s*"([0-9a-f]{64})";'
    else:
        pattern = r'const ' + name + r': u64 =\s*([0-9_]+);'
    matches = re.findall(pattern, source)
    if len(matches) != 1:
        raise ValueError('one consuming media pin required: ' + name)
    return matches[0] if kind in ('sha256', 'text') else int(matches[0].replace('_', ''))


def _relative(name):
    path = PurePosixPath(name)
    if path.is_absolute() or not path.parts or any(part in ('.', '..') for part in path.parts) or '\\' in name or '\x00' in name:
        raise ValueError('contained media resource name required')
    return name


def sound_tag_pin(repo):
    catalog = (Path(repo) / 'core/crates/solstone-core-assets/src/lib.rs').read_text()
    blocks = [block for block in catalog.split('Artifact {') if 'unit: "ced-model"' in block]
    if len(blocks) != 1:
        raise ValueError('one consuming CED model pin required')
    values = {}
    for key, pattern in [('filename', r'filename: "([^"]+)"'),
                         ('sha256', r'sha256: "([0-9a-f]{64})"'),
                         ('size_bytes', r'size_bytes: ([0-9_]+)')]:
        matches = re.findall(pattern, blocks[0].split('\n    },', 1)[0])
        if len(matches) != 1:
            raise ValueError('one consuming CED field required: ' + key)
        values[key] = int(matches[0].replace('_', '')) if key == 'size_bytes' else matches[0]
    if PurePosixPath(_relative(values['filename'])).name != values['filename']:
        raise ValueError('plain CED model filename required')
    return values


def verify_sound_tag_model(repo, directory):
    pin = sound_tag_pin(repo)
    model = Path(directory) / pin['filename']
    if model.is_symlink() or not model.is_file():
        raise ValueError('regular CED model required')
    with model.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if model.stat().st_size != pin['size_bytes'] or digest != pin['sha256']:
        raise ValueError('CED model differs from consuming runtime pin')
    return pin


def verify_sound_tag_resources(repo, directory):
    result = {'model': verify_sound_tag_model(repo, directory), 'notices': []}
    for source, name in SOUND_TAG_NOTICES:
        expected = (Path(repo) / source).read_bytes()
        destination = Path(directory) / 'notices' / name
        if destination.is_symlink() or not destination.is_file() or destination.read_bytes() != expected:
            raise ValueError('packaged CED notice differs: ' + name)
        result['notices'].append({'name': name, 'sha256': hashlib.sha256(expected).hexdigest(), 'bytes': len(expected)})
    return result


def media_package(repo, detector_model, ocr_directory):
    repo = Path(repo)
    rfdetr = (repo / 'tools/mobile-serving/src/rfdetr.rs').read_text()
    ocr = (repo / 'tools/mobile-serving/src/screen_ocr.rs').read_text()
    recipe_path = repo / 'core/screen-ocr-helper/recipe.toml'
    recipe = tomllib.loads(recipe_path.read_text())
    inputs = [(Path(detector_model), _constant(rfdetr, 'PACKAGED_MODEL', 'text'),
               _constant(rfdetr, 'MODEL_SHA256', 'sha256'), _constant(rfdetr, 'MODEL_SIZE', 'size'))]
    for key, constant in [('detector', 'DETECTOR'), ('recognizer', 'RECOGNIZER'), ('dictionary', 'DICTIONARY')]:
        model = recipe['models'][key]
        expected = model.get('sha256', model.get('materialized_sha256'))
        if _constant(ocr, constant + '_SHA256', 'sha256') != expected:
            raise ValueError('mobile OCR model differs from maintained recipe: ' + key)
        name = model['filename']
        if _constant(ocr, constant + '_NAME', 'text') != name:
            raise ValueError('mobile OCR filename differs from maintained recipe: ' + key)
        if PurePosixPath(name).name != name:
            raise ValueError('plain media model filename required')
        inputs.append((Path(ocr_directory) / name, _constant(ocr, 'MODEL_SUBDIR', 'text') + '/' + name,
                       expected, _constant(ocr, constant + '_SIZE', 'size')))
    for source, name in [
        ('core/screen-ocr-helper/NOTICE', 'screen-ocr-NOTICE.txt'),
        ('core/screen-ocr-helper/LICENSE-APACHE', 'Apache-2.0.txt'),
        ('core/distribution/model-notices.md', 'model-NOTICE.md'),
        ('core/distribution/licenses/rfdetr/LICENSE', 'rfdetr-LICENSE.txt'),
    ]:
        inputs.append((repo / source, 'lib/solstone-media-notices/' + name, None, None))
    members = []
    manifest = {'schema': 'solstone.mobile-media.v1', 'recipe_sha256': hashlib.sha256(recipe_path.read_bytes()).hexdigest(), 'files': []}
    for source, name, expected, size in inputs:
        _relative(name)
        if source.is_symlink() or not source.is_file():
            raise ValueError('regular media input required: ' + str(source))
        with source.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        length = source.stat().st_size
        if expected is not None and digest != expected or size is not None and length != size:
            raise ValueError('media input differs from consuming runtime pin: ' + name)
        members.append((source, name))
        manifest['files'].append({'name': name, 'sha256': digest, 'bytes': length})
    return manifest, members


def verify_archive(archive, manifest, prefix):
    for row in manifest['files']:
        name = prefix + row['name']
        infos = [info for info in archive.infolist() if info.filename == name]
        if len(infos) != 1 or infos[0].file_size != row['bytes']:
            raise ValueError('one exact packaged media input required: ' + name)
        with archive.open(infos[0]) as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if digest != row['sha256']:
            raise ValueError('packaged media digest differs: ' + name)
    name = prefix + 'media-models.json'
    if sum(info.filename == name for info in archive.infolist()) != 1 or json.loads(archive.read(name)) != manifest:
        raise ValueError('packaged media manifest differs')


def verify_directory(destination, manifest):
    destination = Path(destination)
    record = destination / 'media-models.json'
    if record.is_symlink() or not record.is_file() or json.loads(record.read_text()) != manifest:
        raise ValueError('packaged media manifest differs')
    for row in manifest['files']:
        target = destination / _relative(row['name'])
        if target.is_symlink() or not target.is_file():
            raise ValueError('regular packaged media input required: ' + row['name'])
        with target.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if target.stat().st_size != row['bytes'] or digest != row['sha256']:
            raise ValueError('packaged media input differs: ' + row['name'])


def verify_payload(repo, destination):
    repo, destination = Path(repo), Path(destination)
    rfdetr = (repo / 'tools/mobile-serving/src/rfdetr.rs').read_text()
    ocr = (repo / 'tools/mobile-serving/src/screen_ocr.rs').read_text()
    manifest, _ = media_package(repo,
        destination / _relative(_constant(rfdetr, 'PACKAGED_MODEL', 'text')),
        destination / _relative(_constant(ocr, 'MODEL_SUBDIR', 'text')))
    verify_directory(destination, manifest)
    return manifest


def stage(destination, manifest, members):
    destination = Path(destination)
    for source, name in members:
        target = destination / _relative(name)
        if target.is_symlink():
            raise ValueError('media staging target is a symbolic link')
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        row = next(row for row in manifest['files'] if row['name'] == name)
        with target.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if target.stat().st_size != row['bytes'] or digest != row['sha256']:
            raise ValueError('staged media input differs: ' + name)
    (destination / 'media-models.json').write_text(json.dumps(manifest, sort_keys=True) + '\n')
    verify_directory(destination, manifest)
