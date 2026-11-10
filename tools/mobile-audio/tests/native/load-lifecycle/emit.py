# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Project exact pinned load bodies into the no-model caller ownership control."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
rows = []
pieces = []
for name, signatures, namespace in (
        ('model.cpp', ('std::unique_ptr<Model> Model::load(',
                       'std::unique_ptr<Model> Model::load_vad_only('), 'pk'),
        ('diarization.cpp', ('std::unique_ptr<DiarizationModel> DiarizationModel::load(',), 'pk'),
        ('parakeet_capi.cpp', ('parakeet_ctx* load_plain(const char* gguf_path) {',), None)):
    path = args.source / 'src' / name
    data = path.read_bytes()
    text = data.decode()
    for signature in signatures:
        # These pinned top-level definitions have column-zero closing braces;
        # nested blocks remain indented. A changed shape must fail compilation.
        assert text.count(signature) == 1, (path, signature)
        start = text.index(signature)
        end = text.index('\n}\n', start) + 3
        body = text[start:end]
        pieces.append(('namespace pk {\n' if namespace else '') + body
                      + ('}\n' if namespace else ''))
        rows.append(dict(source=str(path), source_sha256=hashlib.sha256(data).hexdigest(),
                         signature=signature, body_sha256=hashlib.sha256(body.encode()).hexdigest()))
legacy_metadata = any('global_backend()' in piece for piece in pieces)
assert not (legacy_metadata and any('backend_device_name()' in piece for piece in pieces))
# Select only the explicit caller double appropriate to these exact bodies.
# This is test-only old-revision support, not a native compatibility alias.
args.output.write_text(f'#define LOAD_TEST_LEGACY_METADATA {int(legacy_metadata)}\n' + '\n'.join(pieces))
args.output.with_suffix('.json').write_text(json.dumps(dict(
    scope='Exact source bodies; context/callee doubles are explicitly test-only, no model or OOM proof.',
    projected_sha256=hashlib.sha256(args.output.read_bytes()).hexdigest(), functions=rows), indent=2) + '\n')
