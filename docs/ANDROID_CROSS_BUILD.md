# Android cross-build

The journal workspace can be compiled and linked for `aarch64-linux-android`
with its default features. This is compilation support. Android installation
identity, supervisor admission and readiness remain unsupported and refuse
operation. A running Android journal, app packaging, storage publication,
service lifecycle and release support require separate work.

The build uses Rust 1.97.1, NDK r30 (`30.0.16248370`), arm64-v8a and Android
API31. API31 describes the build coordinate; it does not establish a supported
device floor. Optional features beyond `solstone-core/journal-mcp-endpoint`, tests and installed
runtime behavior are outside this check.

## Reproduce

Use a Linux x86_64 builder with Python 3.12 or newer, CMake 3.31.10, GNU make,
unzip, Git and the repository's pinned Rust toolchain. Keep build scratch on
an on-disk filesystem with enough space for the NDK, native dependencies and
Rust objects. The driver verifies downloaded archives, builds a shared CPU
ONNX Runtime from pinned source, and lets the vendored FFmpeg producer build
its pinned source. Cargo uses the committed lockfile.

```sh
make check-rust-android
```

The native input/build directory defaults to `core/android-build`. To keep
it elsewhere:

```sh
make check-rust-android ANDROID_BUILD_DIR=/var/tmp/journal-android-build
```

The driver writes per-command logs, exits and hashes plus
`receipts/android-cross-build.json` beneath that directory. Rust objects use
this checkout's `core/target-android`, separate from desktop CI. Use an isolated
checkout for each source revision and a new native build directory for a cold
reproduction. No experiment artifact directory is required.

`android-cross-build` is an opt-in platform leg in `core/ci/suites.toml`.
Prepare native inputs outside the contained Rust gate:

```sh
python3 scripts/android_cross_build.py --work-dir core/android-build --prepare-only
```

Select the leg with `make ci-full TARGETS=android-cross-build`. It runs
`check-rust-android-prepared`, which verifies prepared link-input digests and
uses Bash/Cargo without Python. If the native directory is elsewhere, export
`ANDROID_BUILD_DIR` into the gate's environment. Ordinary full CI does not
download an Android toolchain. The check builds
workspace libraries/binaries with Cargo's workspace feature unification and
then builds `solstone-core` with `journal-mcp-endpoint`. It does not establish independent
per-package default-feature coverage or every optional feature.

## Inputs and bindings

The driver pins the NDK and ONNX Runtime archive URLs and SHA-256 digests.
FFmpeg's pin comes from `core/distribution/builder-inputs.toml`; its producer
checks that archive and the committed target bindings. ONNX Runtime's
transitive CMake inputs come from the exact source revision's `cmake/deps.txt`
and are verified by its build system. This build retains CPU/contrib operators
and disables training, CUDA, NNAPI and the ML operator domain. Those settings
are compile/link coverage, not model-execution qualification.

The remaining source-built C dependencies, including SQLite, compression and
cryptography, follow `core/Cargo.lock` and their target-aware Cargo build scripts.
PDFium and external processing/backup executables are loaded at runtime and
are not fabricated as link inputs. PDFium is an upstream binary dependency,
not a source-build claim. This check does not deliver Android runtime payloads
for PDFium, llama.cpp, Parakeet, CED, RF-DETR, restic, rclone or nvattest.

To regenerate the Android FFmpeg bindings after a pin or binding-feature
change, run:

```sh
python3 scripts/android_cross_build.py --work-dir /var/tmp/journal-android-bindings --generate-bindings
```

The generator uses libclang from the pinned NDK.
Commit the resulting `core/vendor/ffmpeg-sys-next/bindings/aarch64-linux-android.rs`
with its recorded source digest. Then run the ordinary Android check without
`--generate-bindings` from a fresh checkout, proving that a normal build uses
the committed file without libclang.
