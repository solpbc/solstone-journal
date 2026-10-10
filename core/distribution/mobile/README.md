# Mobile native builds

Run from a private journal checkout with Python 3.12 or newer, Rust 1.97.1,
CMake 3.31 or newer, Make, Perl, Git and archive tools on PATH. Android requires
Linux x86_64. iOS requires an Apple Silicon Mac with a selected Xcode iPhoneOS SDK.

```sh
make build-mobile-android
```

```sh
make build-mobile-ios
```

Each command creates a fresh build directory under `core/mobile-build` and
reuses only downloads whose SHA-256 matches the maintained inputs. Override
`MOBILE_NATIVE_WORK_ROOT`, `MOBILE_NATIVE_CACHE_DIR` and `MOBILE_NATIVE_JOBS` to
choose storage and concurrency. Keep build storage on disk rather than a RAM
filesystem. The driver refuses an existing work directory.

The build includes all workspace libraries and binaries with default features,
the `journal-mcp-endpoint` feature, Cargo's source-built FFmpeg, ONNX Runtime,
llama.cpp, Parakeet, CED, RF-DETR, restic, rclone, nvattest and PDFium. The iOS
build also includes the Swift Parakeet helper and FluidAudio. FluidAudio uses
Apple CoreML; Android uses the C++ Parakeet engine.

Android uses NDK r30, arm64-v8a and API 31. ONNX Runtime is source-built. iOS
uses device arm64 and minimum iOS 26, with the pinned upstream ONNX Runtime
static XCFramework, including CoreML. PDFium is acquired from the pinned
upstream release on both platforms. These are binary acquisitions and caller
link checks, rather than PDFium or iOS ONNX source compilations.

The C++ recipes preserve Parakeet's embedded CED and voice detection and
llama's OpenSSL HTTPS support. Metal is enabled on iOS; Android uses CPU
backends. The iOS Parakeet server example is excluded because its model-fetch
code invokes subprocesses unavailable on iOS. The engine libraries and caller
link checks remain included. iOS backup tools include Go C archives and real
API callers as well as CLI compilation. nvattest produces its SDK library on
iOS and its CLI plus SDK on Android. The recipes preserve the SDK's crypto
dependencies and trust restrictions.

`inputs.json` fixes external inputs by source coordinate and archive digest.
The nvattest patch is applied to that exact source, with forward and reverse
checks. Parakeet and RF-DETR use their own source-provided GGML patches; the
build checks every patch before CMake can hide a failure. The iOS rclone recipe
adds a build constraint to the pinned Storj dependency's macOS-only CPU
detection file, retaining its portable implementation and the Storj backend.
Cargo locks, Go module checksums and the Swift resolved input retain their
respective dependency graphs.

Receipts are under each run's `build/receipts`. `mobile-native-build.json`
records source identity, input and patch hashes, actual commands and exits,
artifact hashes, architecture inspection and caller linkage. Target callers
are linked, not executed on the build host. `--components` runs diagnostic
subsets; their receipts do not claim a complete build.

Both entrypoints first run the failure controls in [test_build.py](test_build.py).
`make check-mobile-native-driver` runs these without acquiring a toolchain.

The CI registry exposes `mobile-native-android` on Linux and
`mobile-native-ios` on macOS. They are opt-in because they acquire toolchains
and build the whole native stack. Run them after changes to dependency pins,
mobile recipes, patches, consumers or workspace portability, and before
integrating such changes. Their timeout budget is five hours per platform;
actual durations are recorded in the receipts. Normal desktop full CI retains
its existing selection and must also pass before integration.

These checks establish artifact production and caller linkage. They do not
establish mobile app hosting, in-process product integration, model inference,
device endurance, signed packaging or an owner release. Mobile installed
payload identities remain unshipped and are refused by package admission.

See [porting guidance](../../../docs/PORTING.md) for the workspace boundaries.
