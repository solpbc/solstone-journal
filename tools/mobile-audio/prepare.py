#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Verify retained iOS native inputs and stage the maintained audio model."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import urllib.request


def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--retained', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    receipt = json.loads((args.retained / 'receipts/mobile-native-build.json').read_text())
    assert receipt['status'] == 'passed' and receipt['exit'] == 0 and receipt['platform'] == 'ios'
    assert receipt['inputs_sha256'] == digest(repo / 'core/distribution/mobile/inputs.json'), 'native source pins changed; rebuild with scripts/mobile_native_build.py'
    retained = json.loads((args.retained / 'retention.json').read_text())
    args.output.mkdir(parents=True, exist_ok=True)
    records = []
    for entry in retained['artifacts']:
        component = entry['component']
        if not (component.startswith('parakeet-archive-') or component == 'onnx-runtime'):
            continue
        source = Path(entry['retained_path'])
        assert digest(source) == entry['sha256'], f'native artifact digest mismatch: {source}'
        directory = args.output / ('onnx' if component == 'onnx-runtime' else 'parakeet')
        directory.mkdir(exist_ok=True)
        destination = directory / source.name
        shutil.copyfile(source, destination)
        records.append({'path': str(destination), 'sha256': digest(destination), 'source': str(source)})
    assert len(records) == 9, 'expected the maintained Parakeet archive set and ONNX Runtime'
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
        assert digest(partial) == field('sha256'), 'downloaded model digest mismatch'
        partial.rename(model)
    assert digest(model) == field('sha256'), 'Parakeet model digest mismatch'
    for name in ('firered_vad.onnx', 'wespeaker-resnet34-256.onnx', 'pyannote-segmentation-3.0.onnx'):
        shutil.copyfile(repo / 'core/models/assets' / name, model_dir / name)
    notices = args.output / 'notices'
    notices.mkdir(exist_ok=True)
    for source, name in ((repo / 'core/distribution/licenses/fireredvad/LICENSE', 'FireRedVAD-LICENSE.txt'),
                         (repo / 'core/distribution/model-notices.md', 'NOTICE.md')):
        shutil.copyfile(source, notices / name)
    result = {'schema': 'mobile-audio-inputs/v1', 'native': records,
              'models': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(model_dir.iterdir())],
              'notices': [{'path': str(p), 'sha256': digest(p), 'size': p.stat().st_size} for p in sorted(notices.iterdir())]}
    (args.output / 'inputs.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
