# Screen OCR helper

This is an independent Cargo workspace for the private screen OCR worker. It
does not join the main `core/` workspace. `recipe.toml` pins the OCR source,
models, materialized dictionary, helper-private ONNX Runtime archives, OCR
settings, and protocol limits.

The executable accepts only startup paths on its command line:

```text
solstone-screen-ocr-helper --asset-dir <private-model-directory> --runtime-root <private-runtime-directory> --runtime-library <private-onnxruntime-library>
```

It hashes and verifies the pinned detector, recognizer, dictionary, helper, and
explicit runtime library before use. The canonical runtime library path must be
inside the canonical private runtime root, after symlink resolution. It removes
`ORT_DYLIB_PATH` and initializes ONNX Runtime from that exact absolute path. The
ready identity binds the actual runtime-library bytes and recipe digest; it does
not assert a loaded runtime version from the recipe. The helper never resolves
models by network or registry. These paths are startup configuration only; the
stdin/stdout protocol carries no paths, URLs, or environment overrides.

Protocol messages use a 4-byte big-endian header length, a bounded JSON header,
a 4-byte big-endian body length, and a bounded body. The worker writes one ready
identity after model initialization, then processes requests sequentially. A
request body is a PNG; a response body is bounded JSON containing OCR regions.
Request IDs and PNG SHA-256 digests are repeated in responses. Diagnostics are
content-free and stderr is limited to one fixed startup/fatal line.

The helper-owned source is licensed under AGPL-3.0-only. `LICENSE` is copied
verbatim from the repository-root license; `NOTICE` records helper, OAR, and
PP-OCR provenance. OAR and PP-OCR retain their separate Apache-2.0 attributions.
The pinned ORT archives also contain their own `onnxruntime/LICENSE` and
`onnxruntime/ThirdPartyNotices.txt` files for parent/CLO review. The recipe's
runtime archives are acquisition pins, not target qualification. Acquisition,
staging, and install resolution belong to the distribution layer.
