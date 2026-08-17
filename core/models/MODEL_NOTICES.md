# Model notices

This package distributes model weights used by solstone journal-host audio
processing. The package wrapper is AGPL-3.0-only; the model weights retain their
own upstream licenses.

## WeSpeaker ResNet34 speaker embedding model

- Bundled file: `solstone_journal_models/assets/wespeaker-resnet34-256.onnx`
- Upstream model: WeSpeaker ResNet34 speaker embedding model trained on VoxCeleb
- Source artifact: `wespeaker_en_voxceleb_resnet34.onnx` from the k2-fsa/sherpa-onnx `speaker-recongition-models` release
- License: CC-BY-4.0
- SHA-256: `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94`

Redistribution note: CC-BY-4.0 requires attribution to the upstream model
authors and preservation of license notice information when redistributing the
weights.

## pyannote segmentation 3.0

- Bundled file: `solstone_journal_models/assets/pyannote-segmentation-3.0.onnx`
- Upstream model: `pyannote/segmentation-3.0` speaker segmentation model
- Source artifact: `onnx/model.onnx` from `onnx-community/pyannote-segmentation-3.0`
- License: MIT
- SHA-256: `057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25`

MIT License

Copyright (c) 2023 pyannote.audio

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

- Bundled file: `solstone_journal_models/assets/silero_vad_v6.onnx`
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
