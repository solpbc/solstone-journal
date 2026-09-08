# Model notices

Model weights included in the windows payload retain their upstream licenses.

## WeSpeaker ResNet34 speaker embedding model

- Bundled file: `lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx`
- Upstream model: WeSpeaker ResNet34 speaker embedding model trained on VoxCeleb
- Source artifact: `wespeaker_en_voxceleb_resnet34.onnx` from the k2-fsa/sherpa-onnx `speaker-recongition-models` release
- License: CC-BY-4.0
- License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt
- Project: https://github.com/wenet-e2e/wespeaker
- SHA-256: `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94`

Redistribution note: CC-BY-4.0 requires attribution to the upstream model
authors and preservation of license notice information when redistributing the
weights.

## pyannote segmentation 3.0

- Bundled file: `lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx`
- Upstream model: `pyannote/segmentation-3.0` speaker segmentation model
- Source artifact: `onnx/model.onnx` from `onnx-community/pyannote-segmentation-3.0`
- Source: https://huggingface.co/onnx-community/pyannote-segmentation-3.0/tree/733a93b6473d019a773298e08cefa686894b1854
- pyannote.audio license: https://github.com/pyannote/pyannote-audio/blob/3.3.2/LICENSE
- License: MIT
- SHA-256: `057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25`

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

## Silero VAD

- Bundled file: `lib/solstone_journal_models/assets/silero_vad_v6.onnx`
- Upstream project: Silero VAD
- Source: https://github.com/snakers4/silero-vad
- License: MIT
- SHA-256: `4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2`

MIT License

Copyright (c) 2020-present Silero Team

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

## parakeet TDT 0.6B v3 GGUF model

Attribution: parakeet-cpp-gguf (mudler), NVIDIA NeMo Parakeet TDT 0.6B v3.

Source:

- Model repository: https://huggingface.co/mudler/parakeet-cpp-gguf
- Pinned revision: bf0af9f425fa01809cadec671b3cb672709d13e9
- Downloaded file: tdt-0.6b-v3-q8_0.gguf

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

sol pbc redistributes this model under CC-BY-4.0. The attribution above and
the CC-BY-4.0 license URI accompany the redistributed copy.

## RF-DETR nano GGUF model weights

Attribution: RF-DETR (Roboflow); GGUF conversion mudler/rfdetr-cpp-nano.

Source:

- Model repository: https://huggingface.co/mudler/rfdetr-cpp-nano
- Pinned revision: c3dc0c037df499f5503545247df6618415fca643
- Bundled file: `lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf`
- SHA-256: `d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee`

License notice: Apache License 2.0 (Apache-2.0).
