#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Render the declared synthetic audio population for device qualification."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import wave

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--ffmpeg', default='ffmpeg')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
source = Path(__file__).resolve().parents[2] / 'core/fixtures/parakeet_sample.wav'
shutil.copyfile(source, a.output / 'speech.wav')
commands = []
for name, options in [
    ('speech.m4a', ['-i', str(source), '-ar', '44100', '-ac', '2', '-c:a', 'aac', '-b:a', '96k']),
    ('long.wav', ['-stream_loop', '7', '-i', str(source), '-ar', '16000', '-ac', '1', '-c:a', 'pcm_s16le']),
    ('over-duration.m4a', ['-stream_loop', '-1', '-i', str(source), '-t', '121', '-ar', '44100', '-ac', '2', '-c:a', 'aac', '-b:a', '96k']),
]:
    command = [a.ffmpeg, '-nostdin', '-v', 'error', *options, str(a.output / name)]
    subprocess.run(command, check=True, timeout=120)
    commands.append(command)
with wave.open(str(a.output / 'silence.wav'), 'wb') as f:
    f.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
    f.writeframes(bytes(3 * 16000 * 2))
(a.output / 'malformed.wav').write_bytes(b'RIFF\xff\xff\xff\xffWAVEfmt truncated')
with (a.output / 'over-size.wav').open('wb') as f:
    f.write((a.output / 'speech.wav').read_bytes())
    f.truncate(16 * 1024 * 1024 + 1)
manifest = dict(
    expected_words='The quick brown fox jumps over the lazy dog.',
    source=str(source), commands=commands,
    ffmpeg=subprocess.check_output([a.ffmpeg, '-version'], timeout=30, text=True).splitlines()[0],
    files={f.name: dict(bytes=f.stat().st_size, sha256=hashlib.sha256(f.read_bytes()).hexdigest())
           for f in sorted(a.output.iterdir()) if f.is_file()},
)
(a.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(manifest))
