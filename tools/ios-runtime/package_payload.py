#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Stage the maintained router payload as a signed helper bundle resource."""
from pathlib import Path
import shutil
import os

adapter = Path(__file__).resolve().parent
source = adapter.parents[1] / 'core/payload/solstone'
destination = adapter / 'Resources/JournalPayload'
for marker in ('talent/journal/contract/bundle.json', 'think/contract/layout.json',
               'think/templates/segment_preamble.md'):
    assert (source / marker).is_file(), f'missing payload marker: {marker}'
if destination.exists():
    shutil.rmtree(destination)
shutil.copytree(source, destination / 'solstone')
models = adapter / 'Resources/AudioModels'
if models.exists():
    shutil.rmtree(models)
models.mkdir()
notices = models / 'notices'
notices.mkdir()
repo = adapter.parents[1]
shutil.copyfile(repo / 'contracts/journal-web-host/v1.json', adapter / 'Resources/JournalWebHost.json')
shutil.copyfile(repo / 'core/distribution/licenses/fireredvad/LICENSE', notices / 'FireRedVAD-LICENSE.txt')
shutil.copyfile(repo / 'core/distribution/model-notices.md', notices / 'NOTICE.md')
if directory := os.environ.get('AUDIO_MODELS_DIR'):
    for name in ('tdt-0.6b-v3-q8_0.gguf', 'firered_vad.onnx',
                 'wespeaker-resnet34-256.onnx', 'pyannote-segmentation-3.0.onnx'):
        shutil.copyfile(Path(directory) / name, models / name)
