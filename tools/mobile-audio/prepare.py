#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Verify retained iOS native inputs and stage the maintained audio model."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sys
import tempfile
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts'))
from package_mobile_media import sound_tag_pin, verify_sound_tag_model, SOUND_TAG_NOTICES


def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def stage_copy(source, destination):
    if source.is_symlink() or not source.is_file() or destination.parent.is_symlink() or destination.is_symlink():
        raise ValueError('regular audio staging source and destination required')
    temporary = None
    try:
        with source.open('rb') as incoming, tempfile.NamedTemporaryFile(
                dir=destination.parent, prefix='.audio-stage-', delete=False) as output:
            temporary = Path(output.name)
            shutil.copyfileobj(incoming, output)
        os.replace(temporary, destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def verify_audio_pins(receipt, current):
    inputs = receipt.get('inputs', {})
    if not isinstance(inputs, dict):
        raise ValueError('retained audio source pins are missing')
    shared = inputs.get('media-shared-ggml')
    if not isinstance(shared, dict) or shared.get('schema') != 'solstone.mobile-media-ggml.v1':
        raise ValueError('retained audio needs the shared media GGML receipt; rebuild mobile media native inputs')
    # RF-DETR contributes patches to the GGML closure used by both engines.
    # An unrelated local-thinking engine update does not change these inputs.
    names = ('parakeet', 'parakeet-ced', 'parakeet-voice-detect', 'ggml', 'rfdetr')
    for name in names:
        recorded = inputs.get(name)
        if not isinstance(recorded, dict) or not all(
                recorded.get(field) == current[name][field]
                for field in ('url', 'revision', 'sha256')):
            raise ValueError(f'audio source pin changed: {name}')
    expected = {name: current[name]['sha256'] for name in ('ggml', 'parakeet', 'rfdetr')}
    if shared.get('sources') != expected:
        raise ValueError('shared media GGML source pins changed')
    tree = shared.get('tree_sha256')
    if not isinstance(tree, str) or re.fullmatch('[0-9a-f]{64}', tree) is None:
        raise ValueError('shared media GGML tree digest is invalid')
    for name in ('parakeet', 'rfdetr'):
        patched = inputs.get(f'{name}-patched-ggml')
        if name == 'rfdetr' and patched is None:
            continue  # A Parakeet-only build still binds both patch families.
        if not isinstance(patched, dict) or patched.get('tree_sha256') != tree:
            raise ValueError(f'shared media GGML tree differs from the retained {name} build')
    patches = shared.get('patches')
    if not isinstance(patches, list) or not patches:
        raise ValueError('shared media GGML patch list is missing')
    identities = set()
    families = set()
    for patch in patches:
        if not isinstance(patch, dict) or set(patch) != {'source_component', 'path', 'sha256'}:
            raise ValueError('shared media GGML patch record is invalid')
        family, path, checksum = patch['source_component'], patch['path'], patch['sha256']
        if family not in ('parakeet', 'rfdetr'):
            raise ValueError('shared media GGML patch source is invalid')
        if not isinstance(path, str) or not path or '\\' in path or '\0' in path:
            raise ValueError('shared media GGML patch path is invalid')
        relative = PurePosixPath(path)
        if relative.is_absolute() or '..' in relative.parts or relative.suffix != '.patch':
            raise ValueError('shared media GGML patch path is invalid')
        if not isinstance(checksum, str) or re.fullmatch('[0-9a-f]{64}', checksum) is None:
            raise ValueError('shared media GGML patch digest is invalid')
        identity = (family, path)
        if identity in identities:
            raise ValueError('shared media GGML patch record is duplicated')
        identities.add(identity)
        families.add(family)
    if families != {'parakeet', 'rfdetr'}:
        raise ValueError('shared media GGML needs both Parakeet and RF-DETR patch families')
    return shared


def sound_ced_input(retained, repo, current, shared):
    receipt = json.loads((retained / 'receipts/mobile-native-build.json').read_text())
    if receipt.get('status') != 'passed' or receipt.get('exit') != 0 or receipt.get('platform') != 'ios':
        raise ValueError('isolated sound CED needs a successful iOS native receipt')
    if receipt.get('driver_sha256') != digest(repo / 'scripts/mobile_native_build.py'):
        raise ValueError('isolated sound CED driver changed')
    inputs = receipt.get('inputs', {})
    ced = inputs.get('ced', {})
    if any(ced.get(field) != current['ced'][field] for field in ('url', 'revision', 'sha256')):
        raise ValueError('isolated sound CED source pin changed')
    wrapper = inputs.get('sound-ced-wrapper', {})
    expected_wrapper = {
        'sha256': digest(repo / 'core/distribution/mobile/sound-ced/CMakeLists.txt'),
        'source': 'ced', 'symbol_prefix': 'solstone_sound_ced',
        'ggml_tree_sha256': shared['tree_sha256'],
    }
    if wrapper != expected_wrapper or inputs.get('media-shared-ggml') != shared or inputs.get('sound-ced-patched-ggml') != {'tree_sha256': shared['tree_sha256']}:
        raise ValueError('isolated sound CED needs the same verified GGML closure and wrapper')
    produced = [row for row in receipt.get('artifacts', [])
                if str(row.get('component', '')).startswith('sound-ced-archive-')
                and Path(row.get('path', '')).name == 'libsolstone_sound_ced.a']
    retention = json.loads((retained / 'retention.json').read_text())
    kept = [row for row in retention.get('artifacts', [])
            if str(row.get('component', '')).startswith('sound-ced-archive-')
            and Path(row.get('retained_path', '')).name == 'libsolstone_sound_ced.a']
    if len(produced) != 1 or len(kept) != 1:
        raise ValueError('one isolated sound CED archive required')
    source = Path(kept[0]['retained_path'])
    if source.is_symlink() or not source.is_file() or source.stat().st_size <= 0 or source.stat().st_size != produced[0].get('size_bytes') or digest(source) != produced[0].get('sha256') or kept[0].get('sha256') != produced[0].get('sha256'):
        raise ValueError('isolated sound CED retained artifact differs from producer')
    return source, {'source': current['ced'], 'wrapper': wrapper,
                    'driver_sha256': receipt['driver_sha256'], 'sha256': digest(source)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--retained', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model', type=Path)
    parser.add_argument('--ced-model', type=Path)
    parser.add_argument('--sound-ced-retained', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    receipt = json.loads((args.retained / 'receipts/mobile-native-build.json').read_text())
    if receipt.get('status') != 'passed' or receipt.get('exit') != 0 or receipt.get('platform') != 'ios':
        raise ValueError('retained audio needs a successful iOS native build receipt')
    current_pins = json.loads((repo / 'core/distribution/mobile/inputs.json').read_text())['inputs']
    shared_ggml = verify_audio_pins(receipt, current_pins)
    sound_input = sound_ced_input(args.sound_ced_retained, repo, current_pins, shared_ggml) if args.sound_ced_retained else None
    ort_pin = re.search(r'^(?:override\s+)?IOS_ONNX_RUNTIME_LIB_DIGEST\s*[:?]?=\s*([0-9a-f]{64})\s*$',
                        (repo / 'Makefile').read_text(), re.MULTILINE)
    if ort_pin is None:
        raise ValueError('iOS ONNX Runtime digest pin missing')
    retained = json.loads((args.retained / 'retention.json').read_text())
    args.output.mkdir(parents=True, exist_ok=True)
    records = []
    for entry in retained['artifacts']:
        component = entry['component']
        if not (component.startswith('parakeet-archive-') or component == 'onnx-runtime'):
            continue
        source = Path(entry['retained_path'])
        if digest(source) != entry['sha256']:
            raise ValueError(f'native artifact digest mismatch: {source}')
        if component == 'onnx-runtime':
            if entry['sha256'] != ort_pin[1]:
                raise ValueError('iOS ONNX Runtime pin changed')
        directory = args.output / ('onnx' if component == 'onnx-runtime' else 'parakeet')
        directory.mkdir(exist_ok=True)
        destination = directory / source.name
        stage_copy(source, destination)
        records.append({'path': str(destination), 'sha256': digest(destination), 'source': str(source)})
    if len(records) != 9:
        raise ValueError('expected the maintained Parakeet archive set and ONNX Runtime')
    verified_sound = None
    if sound_input:
        source, verified_sound = sound_input
        directory = args.output / 'sound-ced'
        if directory.is_symlink():
            raise ValueError('sound CED directory is a symbolic link')
        directory.mkdir(exist_ok=True)
        if any(entry.name != source.name or entry.is_symlink() or not entry.is_file() for entry in directory.iterdir()):
            raise ValueError('sound CED directory must contain only its isolated archive')
        destination = directory / source.name
        stage_copy(source, destination)
        if digest(destination) != verified_sound['sha256']:
            raise ValueError('staged sound CED artifact changed')
        records.append({'path': str(destination), 'sha256': digest(destination), 'source': str(source)})
    catalog = (repo / 'core/crates/solstone-core-assets/src/lib.rs').read_text()
    block = next(b for b in catalog.split('Artifact {') if 'unit: "parakeet-model"' in b)
    field = lambda key: re.search(rf'{key}: "([^"]+)"', block)[1]
    model_dir = args.output / 'models'
    if model_dir.is_symlink():
        raise ValueError('audio model staging directory is a symbolic link')
    model_dir.mkdir(exist_ok=True)
    (model_dir / 'silero_vad_v6.onnx').unlink(missing_ok=True)
    model = model_dir / field('filename')
    if model.is_symlink():
        raise ValueError('audio model staging destination is a symbolic link')
    if args.model:
        stage_copy(args.model, model)
    elif not model.exists():
        partial = model.with_suffix('.partial')
        if partial.is_symlink():
            raise ValueError('audio model download destination is a symbolic link')
        with urllib.request.urlopen(field('upstream_url'), timeout=120) as source, partial.open('wb') as target:
            shutil.copyfileobj(source, target)
        if digest(partial) != field('sha256'):
            raise ValueError('downloaded model digest mismatch')
        partial.rename(model)
    if digest(model) != field('sha256'):
        raise ValueError('Parakeet model digest mismatch')
    ced = sound_tag_pin(repo)
    ced_source = args.ced_model or repo / 'core/models/assets/ced' / ced['filename']
    if ced_source.is_symlink() or not ced_source.is_file() or ced_source.stat().st_size != ced['size_bytes'] or digest(ced_source) != ced['sha256']:
        raise ValueError('CED model differs from consuming runtime pin')
    stage_copy(ced_source, model_dir / ced['filename'])
    verify_sound_tag_model(repo, model_dir)
    for name in ('firered_vad.onnx', 'wespeaker-resnet34-256.onnx', 'pyannote-segmentation-3.0.onnx'):
        stage_copy(repo / 'core/models/assets' / name, model_dir / name)
    notices = args.output / 'notices'
    if notices.is_symlink():
        raise ValueError('audio notice staging directory is a symbolic link')
    notices.mkdir(exist_ok=True)
    for source, name in ((repo / 'core/distribution/licenses/fireredvad/LICENSE', 'FireRedVAD-LICENSE.txt'),
                         (repo / 'core/distribution/model-notices.md', 'NOTICE.md'),
                         *((repo / source, name) for source, name in SOUND_TAG_NOTICES)):
        stage_copy(source, notices / name)
    result = {'schema': 'mobile-audio-inputs/v1', 'native': records,
              'verified_native_sources': {name: current_pins[name] for name in
                  ('parakeet', 'parakeet-ced', 'parakeet-voice-detect', 'ggml', 'rfdetr')},
              'verified_media_shared_ggml': shared_ggml,
              'verified_sound_ced': verified_sound,
              'models': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(model_dir.iterdir())],
              'notices': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(notices.iterdir())]}
    (args.output / 'inputs.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
