# third-party notices

this file records third-party materials used by solstone, including model
weights bundled with solstone, provider artifacts downloaded at runtime into
the journal provider cache, and test fixtures derived from permissively
licensed sources.

## FFmpeg

The journal statically links FFmpeg for media import and processing. Its
LGPL-2.1-or-later licence text, copyright information, pinned source identity
and source/relink location accompany each journal payload under
`share/licenses/ffmpeg/`. The pinned source is FFmpeg commit
`946fcce07b6dcd0331c8cc609192aeff5e1924f8`; see
`share/licenses/ffmpeg/SOURCE-NOTICE.md` for its archive digest and source URL.

## PDF extraction engine

The journal bundles a native PDFium library from the pinned
`bblanchon/pdfium-binaries` release `chromium/7920`. Its licence and
third-party texts accompany the library under `lib/solstone-core-pdf/`.

### PDFium

Attribution: Google PDFium project.

Source:

- Project: https://pdfium.googlesource.com/pdfium/
- Binary release: https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F7920

License notice: BSD 3-Clause License (BSD-3-Clause).

## bundled model weights

| Bundled file | Upstream model | Source artifact | License | SHA-256 |
|---|---|---|---|---|
| `lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx` | WeSpeaker ResNet34 speaker embedding model trained on VoxCeleb | `wespeaker_en_voxceleb_resnet34.onnx` from the k2-fsa/sherpa-onnx `speaker-recongition-models` release | CC-BY-4.0 | `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94` |
| `lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx` | `pyannote/segmentation-3.0` speaker segmentation model | `onnx/model.onnx` from `onnx-community/pyannote-segmentation-3.0` | MIT | `057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25` |
| `lib/solstone_journal_models/assets/silero_vad_v6.onnx` | Silero VAD voice activity detection model | ONNX model from `snakers4/silero-vad` | MIT | `4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2` |

## bundled test fixtures

### Paradigm Shift AI delayed-video regression fixture

`core/crates/solstone-core-describe/tests/fixtures/delayed_video_probe_screen.mp4`
is a 9.4-second stream-copy subset of source item
`cmcc8u6yc00va1p1ydsdu52zy` from the Computer Use Dataset by Paradigm Shift AI.
the source is `journal/20260201/field.screen/094500_300/screen.mp4` at
`solpbc/field_journal` commit `c1edcc8909f907075916e9ad0f63701da7b607b5`
(SHA-256 `09fa691b99e4d0450922ae39d5be9c16231dd2fa00328d6057c5e0e92e4df6d1`).
the committed subset has SHA-256
`091fa2d732148a0c1e611a72bd320d9db200a790a0fcdf17cfc83d7280d2c17d`.

source:

- Dataset: https://huggingface.co/datasets/anaisleila/computer-use-data-psai
- Provider: Paradigm Shift AI

license notice: MIT License.

Copyright (c) 2025 Paradigm Shift AI
Anais Howland, Ashwin Thinnappan, Jameel Shahid Mohammed

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## provider artifacts (parakeet-cpp)

On linux, sol pbc redistributes these artifacts from `updates.solstone.app` on
demand into the journal provider cache when an owner installs parakeet.cpp. On
windows, the model and sol pbc's own build of the server ship inside the signed
journal package. They are not bundled in this repository.

### parakeet.cpp server binary

Attribution: parakeet.cpp project (mudler).

Source:

- linux release binaries: https://github.com/mudler/parakeet.cpp/releases/tag/v0.5.0
- windows build: sol pbc builds v0.5.0 from source with one patch
  (`core/distribution/parakeet-windows-patches/`). It statically links ggml
  (MIT, https://github.com/ggml-org/ggml), whose license ships with the package.
- Project: https://github.com/mudler/parakeet.cpp

License notice: MIT.

The MIT license permits sol pbc's redistribution of this server binary and its
windows build.

The linux v0.5.0 archives have notice sidecars beside the downloads:

- [parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz.THIRD_PARTY_NOTICES.md)

windows packages built from this source place the YaRN and cpp-httplib blocks at `share/licenses/parakeet/YaRN-LICENSE.txt` and `share/licenses/parakeet/cpp-httplib-LICENSE.txt` beside the parakeet.cpp and ggml licence files. The full text for each component follows.

## parakeet.cpp (linux CLI and server; windows server)

Source: https://github.com/mudler/parakeet.cpp/blob/1bfbebfaaf493866f49597cd3b7901959d395c60/LICENSE

```text
MIT License

Copyright (c) 2026 the parakeet.cpp authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ggml (linux CLI and server; windows server)

Source: https://github.com/ggml-org/ggml/blob/e705c5fed490514458bdd2eaddc43bd098fcce9b/LICENSE

```text
MIT License

Copyright (c) 2023-2026 The ggml authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Mozilla Foundation sgemm.cpp (linux CLI and server)

Source: https://github.com/ggml-org/ggml/blob/e705c5fed490514458bdd2eaddc43bd098fcce9b/src/ggml-cpu/llamafile/sgemm.cpp

```text
Copyright 2024 Mozilla Foundation

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## YaRN (linux CLI and server; windows server)

Source: https://github.com/jquesnelle/yarn/blob/995db5b575e75230b3384d658f8b944c9662f775/LICENSE

```text
MIT License

Copyright (c) 2023 Jeffrey Quesnelle and Bowen Peng

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## cpp-httplib v0.18.3 (linux and windows server only)

Source: https://github.com/yhirose/cpp-httplib/blob/v0.18.3/LICENSE

```text
The MIT License (MIT)

Copyright (c) 2017 yhirose
Copyright (c) 2024 Yuji Hirose. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### parakeet TDT 0.6B v3 GGUF model

Attribution: parakeet-cpp-gguf (mudler), NVIDIA NeMo Parakeet TDT 0.6B v3.

Source:

- Model repository: https://huggingface.co/mudler/parakeet-cpp-gguf
- Pinned revision: bf0af9f425fa01809cadec671b3cb672709d13e9
- Downloaded file: tdt-0.6b-v3-q8_0.gguf

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

sol pbc redistributes this model under CC-BY-4.0. The attribution above and
the CC-BY-4.0 license URI accompany the redistributed copy.

## runtime-downloaded provider artifacts (Parakeet Core ML)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache when an owner installs the Core ML Parakeet
transcription backend. They are not bundled in this repository.

### Parakeet TDT 0.6B v3 Core ML conversion

Attribution: `nvidia/parakeet-tdt-0.6b-v3`.

Source:

- Model repository: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- Pinned revision: `aed02740059203c4a87495924f685de3722ae9ce`

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

The shipped Core ML artifacts are a modified conversion of the source model.
sol pbc redistributes this conversion under CC-BY-4.0 with the attribution,
license URI, and modification indication above.

## runtime-downloaded provider artifacts (local model)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache when an owner installs the local inference
provider. They are not bundled in this repository.

### Qwen3.5-4B GGUF model

Attribution: `unsloth/Qwen3.5-4B-GGUF`, based on Qwen3.5-4B.

Source:

- Model repository: https://huggingface.co/unsloth/Qwen3.5-4B-GGUF
- Downloaded file: `Qwen3.5-4B-Q4_K_M.gguf`
- SHA-256: `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4`
- Downloaded file: `mmproj-F16.gguf`
- SHA-256: `cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model files with its
required notice and license terms.

## runtime-downloaded provider artifacts (ced.cpp sound-tag engine)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache for local ambient sound tagging. They are not
bundled in this repository.

### ced.cpp v0.1.0 engine

Attribution: ced.cpp project (localai-org).

Source:

- Release binaries: https://github.com/localai-org/ced.cpp/releases/tag/v0.1.0
- Project: https://github.com/localai-org/ced.cpp
- Downloaded file: `ced-v0.1.0-lib-linux-cpu-x64.tar.gz`
- SHA-256: `915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360`
- Downloaded file: `ced-v0.1.0-lib-linux-cpu-arm64.tar.gz`
- SHA-256: `a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e`
- Downloaded file: `ced-v0.1.0-lib-macos-metal-arm64.tar.gz`
- SHA-256: `4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b`

License notice: MIT.

The MIT license permits sol pbc's redistribution of this engine binary.

## runtime-downloaded provider artifacts (ced-tiny sound-tag model)

sol pbc redistributes this artifact from `updates.solstone.app` on demand into
the journal provider cache for local ambient sound tagging. It is not bundled
in this repository.

### ced-tiny-q8_0 GGUF model

Attribution: `mudler/ced-gguf`.

Source:

- Model repository: https://huggingface.co/mudler/ced-gguf
- Pinned revision: b5e9a4aad6438763c8da16079d77563fbed35c65
- Downloaded file: `ced-tiny-q8_0.gguf`
- SHA-256: `48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of this model file with its
required notice and license terms.

## runtime-downloaded provider artifacts (rerank cross-encoder)

sol pbc retains these artifacts in the download catalog as a dormant pin. They
are not fetched on POSIX and are not consumed by any product path. They are not
bundled in this repository.

### rerank cross-encoder ONNX model

Attribution: `Xenova/ms-marco-MiniLM-L-6-v2`, an ONNX export of
`cross-encoder/ms-marco-MiniLM-L-6-v2`.

Source:

- Model repository: https://huggingface.co/Xenova/ms-marco-MiniLM-L-6-v2
- Pinned revision: a09144355adeed5f58c8ed011d209bf8ee5a1fec
- Downloaded files: `onnx/model.onnx`, `tokenizer.json`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model files with its
required notice and license terms.

## bundled RF-DETR object-detection assets (rf-detr.cpp)

sol pbc bundles these artifacts in every journal release and installs them from
the release tree into the journal provider cache for local object detection.
They are not fetched at install time.

### rf-detr.cpp v0.1.0-solpbc.5 engine

Attribution: rf-detr.cpp (Ettore Di Giacinto / mudler); binaries CI-built and
released by sol pbc.

Source:

- Release binaries: https://github.com/solpbc/rf-detr.cpp/releases/tag/v0.1.0-solpbc.5
- Project: https://github.com/localai-org/rf-detr.cpp
- Pinned engine ref: ec73712e
- Release tag: v0.1.0-solpbc.5
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz`
- SHA-256: `56231d6675395ed790dba882e0335e4c79616427af558b1820975951cd9d14a7`
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz`
- SHA-256: `2c11e1af6986571d4d9f4d2cf377018973095b10c234a9da40a3edf45cf11f9d`
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz`
- SHA-256: `46b497950c7a73000007abdb9ef54bc8b46ba0a46dcf26f6c0ae51fccd21ad71`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these engine binaries with their
required notice and license terms.

### RF-DETR nano GGUF model weights

Attribution: RF-DETR (Roboflow); GGUF conversion mudler/rfdetr-cpp-nano.

Source:

- Model repository: https://huggingface.co/mudler/rfdetr-cpp-nano
- Pinned revision: c3dc0c037df499f5503545247df6618415fca643
- Bundled file: `rfdetr-nano-f16.gguf`
- SHA-256: `d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model weights with its
required notice and license terms.

## WeSpeaker ResNet34 / VoxCeleb

Attribution: WeSpeaker project, ResNet34 speaker embedding model trained on
VoxCeleb.

Source:

- Exact bundled artifact:
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx
- Release checksum file:
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/checksum.txt
- WeSpeaker project:
  https://github.com/wenet-e2e/wespeaker
- WeSpeaker pretrained-model license note:
  https://github.com/wenet-e2e/wespeaker/blob/master/docs/pretrained.md#model-license

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
WeSpeaker's pretrained-model documentation states that pretrained models follow
the license of the corresponding dataset, and that pretrained models on VoxCeleb
follow Creative Commons Attribution 4.0 International because VoxCeleb uses that
license. License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

## pyannote segmentation 3.0

Attribution: pyannote.audio project, `pyannote/segmentation-3.0` speaker
segmentation model.

Source:

- Exact bundled ONNX artifact:
  https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/main/onnx/model.onnx
- ONNX-community model card:
  https://huggingface.co/onnx-community/pyannote-segmentation-3.0
- Original pyannote model card:
  https://huggingface.co/pyannote/segmentation-3.0
- pyannote.audio source:
  https://github.com/pyannote/pyannote-audio

License notice: MIT. The retained MIT notice follows.

```text
MIT License

Copyright (c) 2020 CNRS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## runtime-downloaded provider artifacts (llama.cpp CUDA)

These artifacts are downloaded on demand into the journal provider cache for
solstone's local inference runtime on supported NVIDIA GPU systems. They are
distributed as application components, not as a stand-alone CUDA distribution.
sol pbc redistributes them from `updates.solstone.app`; they are not bundled in
this repository.

### llama.cpp and ggml runtime

Files: `llama-server`, `libllama-server-impl.so`,
`libllama-common.so.0`, `libmtmd.so.0`, `libllama.so.0`,
`libggml.so.0`, `libggml-base.so.0`, `libggml-cuda.so`, and the
architecture-specific `libggml-cpu-*.so` files.

Source: https://github.com/ggml-org/llama.cpp

License: MIT License.

The complete llama.cpp MIT license and copyright notice is reproduced in
`licenses/llama.cpp-LICENSE.txt` and accompanies each runtime artifact.
The MIT license permits sol pbc's redistribution of these llama.cpp runtime
files.

### NVIDIA CUDA runtime components

Files: `libcudart.so.13`, `libcublas.so.13`,
`libcublasLt.so.13`.

Source:
- linux x86_64 archive `llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz` and linux arm64 archive `llama-b11429-bin-linux-cuda13-arm64-sol1.tar.gz` each carry CUDA Toolkit 13.4.1 components. Each archive is packaged with the MIT license file `licenses/llama.cpp-LICENSE.txt` and the unmodified EULA at `licenses/NVIDIA-CUDA-EULA-13.4.1.html`. The archive root also contains `provenance.json`.

License: NVIDIA CUDA Toolkit End User License Agreement, Release 13.4.1
for linux x86_64 and linux arm64, including the CUDA Toolkit
Supplement, Attachment A, and Attachment B.

These files are proprietary NVIDIA software. They are not licensed under
solstone's AGPL-3.0-only license or the llama.cpp MIT license. Their use and
redistribution remain subject to the NVIDIA CUDA Toolkit EULA. A verbatim
copy of the unmodified 13.4.1 EULA accompanies each selected linux archive at
`licenses/NVIDIA-CUDA-EULA-13.4.1.html`. NVIDIA does not sponsor or endorse solstone.

sol pbc redistributes these CUDA components from `updates.solstone.app` only as
Attachment-A distributable portions: unmodified except for unzipping, inside
the solstone application with material additional functionality, and not as a
stand-alone SDK distribution. Their redistribution is permitted only within
those NVIDIA CUDA Toolkit EULA bounds.


### llama.cpp hash components

the llama.cpp b11429 runtime includes the following hash components.
their upstream license texts are reproduced below.

Source: https://github.com/ggml-org/llama.cpp/tree/d81235049384534c167caea52b85a694f6103d14/vendor/hash

#### xxHash

Copyright (C) 2012-2023 Yann Collet

xxHash Library
Copyright (c) 2012-2021 Yann Collet
All rights reserved.

BSD 2-Clause License (https://www.opensource.org/licenses/bsd-license.php)

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#### rotate-bits

MIT License

Copyright (c) 2021 William Casarin <jb55@jb55.com>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

#### SHA-256

2010-06-11 : Igor Pavlov : Public domain

#### SHA-1

the upstream source carries this attribution:

```text
SHA-1 in C
By Steve Reid <steve@edmweb.com>
100% Public Domain
```

## runtime-downloaded provider artifacts (llama.cpp Vulkan/CPU)

These artifacts are downloaded on demand into the journal provider cache for
solstone's local inference runtime on supported macos and linux systems. They
are distributed as application components, not as stand-alone runtime
distributions.
sol pbc redistributes them from `updates.solstone.app`; they are not bundled in
this repository.

### llama.cpp Vulkan/CPU runtime

Files: `llama-server`, extracted from
`llama-b11429-bin-macos-arm64.tar.gz`,
`llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz`, and
`llama-b11429-bin-ubuntu-vulkan-x64.tar.gz`.

Source: https://github.com/ggml-org/llama.cpp

License: MIT License.

The complete llama.cpp MIT license and copyright notice is reproduced in
`licenses/llama.cpp-LICENSE.txt` and accompanies each runtime artifact.
The MIT license permits sol pbc's redistribution of these Vulkan/CPU runtime
files.

## Microsoft Visual C++ runtime (windows)

The windows package includes unmodified Microsoft redistributable runtime
libraries in its application directory. They are not part of solstone and are
not covered by solstone's AGPL-3.0-only license.

Attribution: Microsoft Corporation.

Source:

- Package: Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix
  (SHA-256 4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5)
  from download.visualstudio.microsoft.com
- Toolset: Microsoft.VC143.CRT and Microsoft.VC143.OpenMP, 14.44.35112

| Bundled file | SHA-256 |
|---|---|
| `bin/msvcp140.dll`      | `0f885b509a685d2bbfa652fed26b5fb31d88fbdab0a978c641d1c7b8aa460aa9` |
| `bin/vcruntime140.dll`  | `d5e4d9a3e835fa679450145d6a7d94e36573a509317111904d9b3712c30d9066` |
| `bin/vcruntime140_1.dll`| `1f2d41c4aa5db0bc33ebf7b66d72943a817d7ce6cbe880502a9403823633093f` |
| `bin/vcomp140.dll`      | `55aba23cdcd6484fbb06f4155b8ca75adfce7a881f10afd0c49457165e677164` |

License notice: sol pbc redistributes these components under the Distributable
Code terms of the Microsoft Visual Studio 2022 license, published at
https://visualstudio.microsoft.com/license-terms/ (Distributable List:
https://aka.ms/vs/17/redistribution). They are licensed separately by
Microsoft. You may redistribute them only as part of this program, unmodified,
and you may not reverse engineer, decompile or disassemble them, or remove or
alter any Microsoft notice they carry. Microsoft provides these files as is and
gives no warranty for them. Microsoft does not sponsor or endorse solstone.
The same restrictions are restated in the installed package at
share/licenses/msvc/NOTICE.md.

## Microsoft Edge WebView2 loader (windows)

The journal app for windows (`bin/journal-app.exe`) embeds the Microsoft Edge
WebView2 loader, linked statically. The loader is not part of solstone and is
not covered by solstone's AGPL-3.0-only license.

Attribution: Microsoft Corporation.

Source:

- Package: Microsoft.Web.WebView2 1.0.3650.58 from nuget.org
  (SHA-256 911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60),
  member build/native/x64/WebView2LoaderStatic.lib
- Carried by: the webview2-com-sys 0.38.2 crate, member
  x64/WebView2LoaderStatic.lib, byte-identical to that package member

| Linked library | SHA-256 |
|---|---|
| `WebView2LoaderStatic.lib` (x64) | `0659b741bde6348d4c4a6ec4ceb9af50e3d0048ed9cd3c8659bccbb61fde55ee` |

License notice: BSD 3-Clause License (BSD-3-Clause), from LICENSE.txt in the
NuGet package. The webview2-com-sys crate's MIT license does not cover this
library. The complete license text is reproduced in the installed package at
share/licenses/webview2-loader/NOTICE.md.
