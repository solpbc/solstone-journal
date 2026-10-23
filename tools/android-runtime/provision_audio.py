#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Provision pinned native models into a separate development application's files."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--adb',default='adb');p.add_argument('--serial',required=True)
p.add_argument('--package',default='app.solstone.development.journal.audio')
p.add_argument('--parakeet',type=pathlib.Path)
p.add_argument('--voice-only',action='store_true')
a=p.parse_args()
if a.package not in {'app.solstone.development.journal.audio', 'app.solstone.development.journal.services'}: p.error('audio development application required')
if (a.parakeet is None) == (not a.voice_only): p.error('choose --parakeet or --voice-only')
repo=pathlib.Path(__file__).resolve().parents[2]
catalog=(repo/'core/crates/solstone-core-assets/src/lib.rs').read_text()
block=next(b for b in catalog.split('Artifact {') if 'unit: "parakeet-model"' in b)
field=lambda key:re.search(rf'{key}: "([^"]+)"',block)[1]
if a.parakeet:
 if a.parakeet.name!=field('filename'):p.error('maintained model filename required')
 with a.parakeet.open('rb') as f:
  if hashlib.file_digest(f,'sha256').hexdigest()!=field('sha256'):p.error('model differs from maintained catalog pin')
models=([a.parakeet] if a.parakeet else [])+[repo/'core/models/assets'/name for name in ['firered_vad.onnx','wespeaker-resnet34-256.onnx','pyannote-segmentation-3.0.onnx']]
adb=[a.adb,'-s',a.serial]
def run(*argv):return subprocess.check_output([*adb,*argv],stderr=subprocess.STDOUT,timeout=300).decode()
run('shell','am','force-stop',a.package)
run('shell','run-as',a.package,'mkdir','-p','files/audio-models/notices')
run('shell','run-as',a.package,'rm','-f','files/audio-models/silero_vad_v6.onnx')
staging='/data/local/tmp/journal-audio-model-provision-'+a.package
run('shell','mkdir','-p',staging)
receipts=[]
notice_receipts=[]
payloads=[(model,'files/audio-models/'+model.name,receipts) for model in models]+[
 (repo/'core/distribution/licenses/fireredvad/LICENSE','files/audio-models/notices/FireRedVAD-LICENSE.txt',notice_receipts),
 (repo/'core/distribution/model-notices.md','files/audio-models/notices/NOTICE.md',notice_receipts),
]
try:
 for source,target,records in payloads:
  with source.open('rb') as handle:sha=hashlib.file_digest(handle,'sha256').hexdigest()
  run('push',str(source),staging+'/'+source.name)
  run('shell','run-as',a.package,'cp',staging+'/'+source.name,target)
  actual=run('shell','run-as',a.package,'sha256sum',target).split()[0]
  if actual!=sha:raise RuntimeError('payload transfer digest mismatch: '+source.name)
  records.append(dict(name=pathlib.Path(target).name,sha256=sha,bytes=source.stat().st_size))
finally:
 for source,_,_ in payloads:run('shell','rm','-f',staging+'/'+source.name)
 run('shell','rmdir',staging)
print(json.dumps(dict(package=a.package,serial=a.serial,models=receipts,notices=notice_receipts)))
