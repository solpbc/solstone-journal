#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Verify retained iOS native inputs and stage the maintained audio model."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import urllib.request


def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--retained', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    receipt = json.loads((args.retained / 'receipts/mobile-native-build.json').read_text())
    if receipt.get('status') != 'passed' or receipt.get('exit') != 0 or receipt.get('platform') != 'ios':
        raise ValueError('retained audio needs a successful iOS native build receipt')
    current_pins = json.loads((repo / 'core/distribution/mobile/inputs.json').read_text())['inputs']
    shared_ggml = verify_audio_pins(receipt, current_pins)
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
        shutil.copyfile(source, destination)
        records.append({'path': str(destination), 'sha256': digest(destination), 'source': str(source)})
    if len(records) != 9:
        raise ValueError('expected the maintained Parakeet archive set and ONNX Runtime')
    catalog = (repo / 'core/crates/solstone-core-assets/src/lib.rs').read_text()
    block = next(b for b in catalog.split('Artifact {') if 'unit: "parakeet-model"' in b)
    field = lambda key: re.search(rf'{key}: "([^"]+)"', block)[1]
    model_dir = args.output / 'models'
    model_dir.mkdir(exist_ok=True)
    (model_dir / 'silero_vad_v6.onnx').unlink(missing_ok=True)
    model = model_dir / field('filename')
    if args.model:
        shutil.copyfile(args.model, model)
    elif not model.exists():
        partial = model.with_suffix('.partial')
        with urllib.request.urlopen(field('upstream_url'), timeout=120) as source, partial.open('wb') as target:
            shutil.copyfileobj(source, target)
        if digest(partial) != field('sha256'):
            raise ValueError('downloaded model digest mismatch')
        partial.rename(model)
    if digest(model) != field('sha256'):
        raise ValueError('Parakeet model digest mismatch')
    for name in ('firered_vad.onnx', 'wespeaker-resnet34-256.onnx', 'pyannote-segmentation-3.0.onnx'):
        shutil.copyfile(repo / 'core/models/assets' / name, model_dir / name)
    notices = args.output / 'notices'
    notices.mkdir(exist_ok=True)
    for source, name in ((repo / 'core/distribution/licenses/fireredvad/LICENSE', 'FireRedVAD-LICENSE.txt'),
                         (repo / 'core/distribution/model-notices.md', 'NOTICE.md')):
        shutil.copyfile(source, notices / name)
    result = {'schema': 'mobile-audio-inputs/v1', 'native': records,
              'verified_native_sources': {name: current_pins[name] for name in
                  ('parakeet', 'parakeet-ced', 'parakeet-voice-detect', 'ggml', 'rfdetr')},
              'verified_media_shared_ggml': shared_ggml,
              'models': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(model_dir.iterdir())],
              'notices': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(notices.iterdir())]}
    (args.output / 'inputs.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
