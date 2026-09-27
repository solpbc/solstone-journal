[![ffmpeg-sys-next on crates.io](https://img.shields.io/crates/v/ffmpeg-sys-next?cacheSeconds=3600)](https://crates.io/crates/ffmpeg-sys-next)
[![build](https://github.com/zmwangx/rust-ffmpeg-sys/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/zmwangx/rust-ffmpeg-sys/actions)

This is a fork of the abandoned [ffmpeg-sys](https://github.com/meh/rust-ffmpeg-sys) crate. You can find this crate as [ffmpeg-sys-next](https://crates.io/crates/ffmpeg-sys-next) on crates.io.

This crate contains low level bindings to FFmpeg. You're probably interested in the high level bindings instead: [ffmpeg-next](https://github.com/zmwangx/rust-ffmpeg).

A word on versioning: major and minor versions track major and minor versions of FFmpeg, e.g. 4.2.x of this crate has been updated to support the 4.2.x series of FFmpeg. Patch level is reserved for bug fixes of this crate and does not track FFmpeg patch versions.

## Feature flags

In addition to feature flags declared in `Cargo.toml`, this crate performs various compile-time version and feature detections and exposes the results in additional flags. These flags are briefly documented below; run `cargo build -vv` to view more details.

- `ffmpeg_<x>_<y>` flags (new in v4.3.2), e.g. `ffmpeg_4_4`, indicating the FFmpeg installation being compiled against is at least version `<x>.<y>`. Currently available:

  - `ffmpeg_3_0`
  - `ffmpeg_3_1`
  - `ffmpeg_3_2`
  - `ffmpeg_3_3`
  - `ffmpeg_3_4`
  - `ffmpeg_4_0`
  - `ffmpeg_4_1`
  - `ffmpeg_4_2`
  - `ffmpeg_4_3`
  - `ffmpeg_4_4`
  - `ffmpeg_5_0`
  - `ffmpeg_5_1`
  - `ffmpeg_6_0`
  - `ffmpeg_6_1`
  - `ffmpeg_7_0`
  - `ffmpeg_7_1`
  - `ffmpeg_8_0`
  - `ffmpeg_8_1`
  - `ffmpeg_9_0`

- `avcodec_version_greater_than_<x>_<y>` (new in v4.3.2), e.g., `avcodec_version_greater_than_58_90`. The name should be self-explanatory.

- `ff_api_<feature>`, e.g. `ff_api_vaapi`, corresponding to whether their respective uppercase deprecation guards evaluate to true.

- `ff_api_<feature>_is_defined`, e.g. `ff_api_vappi_is_defined`, similar to above except these are enabled as long as the corresponding deprecation guards are defined.

## Committed bindings

With `default-features = false` and the `build` feature, this vendored copy builds FFmpeg from the pinned source in `core/distribution/builder-inputs.toml` and does not run bindgen on Linux or macOS. It installs the file in `bindings/` for the target triple instead, so those builds need neither bindgen nor libclang. Each file's first line records its target and the SHA-256 of the FFmpeg source archive it was generated from, and the build refuses a file that does not match the current pin.

Windows still generates its bindings with bindgen at build time. So does any build that enables the `generate-bindings` feature, which is required for a prebuilt FFmpeg or for the `avdevice`, `avfilter`, `postproc`, `avresample` or `non-exhaustive-enums` features.

After the FFmpeg pin moves, regenerate every committed file with the pinned archive in place (`solstone-distribution acquire ffmpeg`):

```sh
solstone-distribution ffmpeg-bindings linux-x86_64    # on Linux, with zig
solstone-distribution ffmpeg-bindings linux-aarch64   # on Linux, with zig
solstone-distribution ffmpeg-bindings macos-arm64     # on macOS; also writes aarch64-apple-ios
```

Each command builds FFmpeg for that target's Rust triples with the same toolchains as a release build and rewrites their files.
