# Mobile audio adapter

This development adapter calls the maintained Parakeet native C API in process. It connects the model to the production embedded transcription pipeline without a subprocess or network inference service. The caller owns the worker thread and supplies app-writable scratch storage.

`Parakeet::new(model, scratch)` verifies the model size and SHA-256 against the existing asset catalog before loading it. Its `EmbeddedAsr` implementation returns the verbose word/timestamp contract consumed by the production transcription parser. The context stays on its creating thread. The adapter re-exports `process_embedded` and `discover_audio_files` for orchestration.

Set `PARAKEET_LIB_DIR` to a directory containing the maintained static Parakeet and GGML archives. Native Android and iOS builds also require `SOUND_CED_LIB_DIR`, containing only `libsolstone_sound_ced.a` from the `sound-ced` native recipe. Parakeet supplies the shared GGML libraries. iOS links the Apple frameworks used by that build; Android links its static C++ runtime and ABI library. `--no-default-features` permits host contract tests without native archives.

[prepare.py](prepare.py) stages iOS libraries from a successful retained build receipt, verifies their hashes, downloads and verifies the pinned Parakeet model, and copies the VAD, speaker and CED models with their notices. Supply `--ced-model` to use a local copy of the catalog-pinned CED model; the default is `core/models/assets/ced/ced-tiny-q8_0.gguf`. Pass `--sound-ced-retained` to add the isolated CED archive from a current-driver receipt with the same GGML binding. The script writes an `inputs.json` receipt. The [iOS runtime guide](../ios-runtime/README.md#local-audio-processing) documents installation and physical qualification.
