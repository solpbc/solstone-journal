# third-party notices

this file records third-party materials used by solstone, including model
weights bundled with solstone, provider artifacts downloaded at runtime into
the journal provider cache, and test fixtures derived from permissively
licensed sources.

<!-- Six runtime sections previously said "not bundled"; CUDA and Vulkan/CPU
used application-component wording. Each section below states its own redistribution terms. -->

## PDF extraction engine

The PDF import worker uses pypdfium2, whose wheel bundles Google's PDFium
library. These artifacts are installed as Python package dependencies; they are
not source code owned by solstone.

### pypdfium2

Attribution: pypdfium2 project.

Source:

- Project: https://github.com/pypdfium2-team/pypdfium2
- Package: https://pypi.org/project/pypdfium2/

License notice: BSD 3-Clause License (BSD-3-Clause).

### PDFium

Attribution: Google PDFium project.

Source:

- Project: https://pdfium.googlesource.com/pdfium/

License notice: BSD 3-Clause License (BSD-3-Clause).

## bundled model weights

| Bundled file | Upstream model | Source artifact | License | SHA-256 |
|---|---|---|---|---|
| `solstone_journal_models/assets/wespeaker-resnet34-256.onnx` | WeSpeaker ResNet34 speaker embedding model trained on VoxCeleb | `wespeaker_en_voxceleb_resnet34.onnx` from the k2-fsa/sherpa-onnx `speaker-recongition-models` release | CC-BY-4.0 | `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94` |
| `solstone_journal_models/assets/pyannote-segmentation-3.0.onnx` | `pyannote/segmentation-3.0` speaker segmentation model | `onnx/model.onnx` from `onnx-community/pyannote-segmentation-3.0` | MIT | `057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25` |
| `solstone_journal_models/assets/silero_vad_v6.onnx` | Silero VAD voice activity detection model | ONNX model from `snakers4/silero-vad` | MIT | `4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2` |

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

## runtime-downloaded provider artifacts (parakeet-cpp)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache when an owner opts into the `parakeet-cpp`
transcription backend. They are not bundled in this repository.

### parakeet.cpp server binary

Attribution: parakeet.cpp project (mudler).

Source:

- Release binaries: https://github.com/mudler/parakeet.cpp/releases/tag/v0.5.0
- Project: https://github.com/mudler/parakeet.cpp

License notice: MIT.

The MIT license permits sol pbc's redistribution of this server binary.

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

## runtime-downloaded provider artifacts (rf-detr.cpp)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache for local object detection. They are not
bundled in this repository.

### rf-detr.cpp v0.1.0-solpbc.5 engine

Attribution: rf-detr.cpp (Ettore Di Giacinto / mudler); binaries CI-built and
released by sol pbc.

Source:

- Release binaries: https://github.com/solpbc/rf-detr.cpp/releases/tag/v0.1.0-solpbc.5
- Project: https://github.com/localai-org/rf-detr.cpp
- Pinned engine ref: ec73712e
- Release tag: v0.1.0-solpbc.5
- Downloaded file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz`
- SHA-256: `56231d6675395ed790dba882e0335e4c79616427af558b1820975951cd9d14a7`
- Downloaded file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz`
- SHA-256: `2c11e1af6986571d4d9f4d2cf377018973095b10c234a9da40a3edf45cf11f9d`
- Downloaded file: `rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz`
- SHA-256: `46b497950c7a73000007abdb9ef54bc8b46ba0a46dcf26f6c0ae51fccd21ad71`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these engine binaries with their
required notice and license terms.

### RF-DETR nano GGUF model weights

Attribution: RF-DETR (Roboflow); GGUF conversion mudler/rfdetr-cpp-nano.

Source:

- Model repository: https://huggingface.co/mudler/rfdetr-cpp-nano
- Pinned revision: c3dc0c037df499f5503545247df6618415fca643
- Downloaded file: `rfdetr-nano-f16.gguf`

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

Source: NVIDIA CUDA Toolkit 13.3 packages contained in the pinned upstream
llama.cpp CUDA image.

License: NVIDIA CUDA Toolkit End User License Agreement, Release 13.3,
including the CUDA Toolkit Supplement, Attachment A, and Attachment B.

These files are proprietary NVIDIA software. They are not licensed under
solstone's AGPL-3.0 license or the llama.cpp MIT license. Their use and
redistribution remain subject to the NVIDIA CUDA Toolkit EULA. A verbatim
copy of the package-accompanying EULA, including its third-party notices,
is reproduced in `licenses/NVIDIA-CUDA-EULA-13.3.txt` and accompanies each
runtime artifact. NVIDIA does not sponsor or endorse solstone.

sol pbc redistributes these CUDA components from `updates.solstone.app` only as
Attachment-A distributable portions: unmodified except for unzipping, inside
the solstone application with material additional functionality, and not as a
stand-alone SDK distribution. Their redistribution is permitted only within
those NVIDIA CUDA Toolkit EULA 13.3 bounds.

## runtime-downloaded provider artifacts (llama.cpp Vulkan/CPU)

These artifacts are downloaded on demand into the journal provider cache for
solstone's local inference runtime on supported macOS and Linux systems. They
are distributed as application components, not as stand-alone runtime
distributions.
sol pbc redistributes them from `updates.solstone.app`; they are not bundled in
this repository.

### llama.cpp Vulkan/CPU runtime

Files: `llama-server`, extracted from
`llama-b10068-bin-macos-arm64.tar.gz`,
`llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz`, and
`llama-b10068-bin-ubuntu-vulkan-x64.tar.gz`.

Source: https://github.com/ggml-org/llama.cpp

License: MIT License.

The complete llama.cpp MIT license and copyright notice is reproduced in
`licenses/llama.cpp-LICENSE.txt` and accompanies each runtime artifact.
The MIT license permits sol pbc's redistribution of these Vulkan/CPU runtime
files.
