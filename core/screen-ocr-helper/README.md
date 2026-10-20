# Screen OCR helper

This is an independent Cargo workspace for the private screen OCR worker. It
does not join the main `core/` workspace. `recipe.toml` pins the OCR source,
models, materialized dictionary, helper-private ONNX Runtime archives, OCR
settings, and protocol limits.

The executable accepts only startup paths on its command line:

```text
solstone-screen-ocr-helper --asset-dir <private-model-directory> --runtime-root <private-runtime-directory> --runtime-library <private-onnxruntime-library>
```

It verifies the pinned detector, recognizer and dictionary before use. It hashes
its executable and explicit runtime library into the ready identity. The journal
verifies installed members and compares the complete ready identity before
accepting recognized text. The canonical runtime library path must be
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
The staged runtime includes its archive's `onnxruntime/LICENSE` and
`onnxruntime/ThirdPartyNotices.txt`. The recipe's
runtime archives are acquisition pins, not target qualification. Acquisition,
staging, and install resolution belong to the distribution layer.

Build and dependency-source instructions are in [SOURCE-DIRECTIONS.md](SOURCE-DIRECTIONS.md).

The journal runs the helper automatically for retained frames whose known
primary or secondary category is not ignored. Its per-source private sidecar
records coverage and OCR/Qwen word counts for each frame. The public
`talents/screen_text.md` record contains first-seen lines after deterministic
filtering and best-effort credential masking. Written model redaction rules do
not apply to that record. The record says what was visible, without claiming
what the owner read, typed or did.

Offsets use the existing describe/raw-screen timeline: decoded PTS multiplied
by the time base is added to the segment start. A nonzero first retained PTS is
preserved. Current capture and ingest records provide no additional per-video
origin; the journal does not guess one or infer typing from a text change.

The installed resolver currently admits linux x86_64 and macos arm64. windows
x86_64 and linux AArch64 remain unavailable until native qualification. A
missing or failing helper records an OCR gap and leaves model-based screen
processing unchanged. OCR words remain outside ordinary model context and
daily evidence. The private sidecar retains occurrences and geometry; the
public record uses the separate screen-text agent permission.
