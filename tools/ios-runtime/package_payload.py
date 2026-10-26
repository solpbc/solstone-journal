#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Stage the maintained router payload as a signed helper bundle resource."""
from pathlib import Path
import shutil
import os
import sys

adapter = Path(__file__).resolve().parent
repo = adapter.parents[1]
sys.path.insert(0,str(repo/'scripts'))
import package_mobile_media
ocr_directory = os.environ.get('SCREEN_OCR_MODELS_DIR')
if os.environ.get('REQUIRE_MEDIA_MODELS') == '1' and not ocr_directory:
    raise ValueError('owner builds require SCREEN_OCR_MODELS_DIR')
media = package_mobile_media.media_package(repo, os.environ.get('RFDETR_MODEL') or repo/'core/models/assets/rfdetr/rfdetr-nano-f16.gguf', ocr_directory) if ocr_directory else None
source = repo / 'core/payload/solstone'
destination = adapter / 'Resources/JournalPayload'
for marker in ('talent/journal/contract/bundle.json', 'think/contract/layout.json',
               'think/templates/segment_preamble.md'):
    if not (source / marker).is_file():
        raise ValueError(f'missing payload marker: {marker}')
if destination.exists():
    shutil.rmtree(destination)
shutil.copytree(source, destination / 'solstone')
if media:
    package_mobile_media.stage(destination, *media)
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
for source, name in package_mobile_media.SOUND_TAG_NOTICES:
    shutil.copyfile(repo / source, notices / name)
if directory := os.environ.get('AUDIO_MODELS_DIR'):
    ced = package_mobile_media.verify_sound_tag_model(repo, directory)
    for name in ('tdt-0.6b-v3-q8_0.gguf', 'firered_vad.onnx',
                 'wespeaker-resnet34-256.onnx', 'pyannote-segmentation-3.0.onnx', ced['filename']):
        shutil.copyfile(Path(directory) / name, models / name)
    package_mobile_media.verify_sound_tag_resources(repo, models)
