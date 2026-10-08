# CED sound-tag engine and model

- **Engine:** ced.cpp v0.1.0 (source commit `c04ac14b7992d00584d9e812c9bb6268598a6ce7`), MIT License, see `LICENSE`. It statically includes ggml (commit `e705c5fed490514458bdd2eaddc43bd098fcce9b`), MIT License, see `ggml-LICENSE`.
- **Model:** `ced-tiny-q8_0.gguf` from `mudler/ced-gguf` at revision `b5e9a4aad6438763c8da16079d77563fbed35c65`, Apache License 2.0, see `ced-tiny-model-LICENSE`. It is a GGUF quantization of the `mispeech/ced-tiny` checkpoint (CED, Consistent Ensemble Distillation, Xiaomi), which is also published under the Apache License 2.0.
