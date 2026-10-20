<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Screen OCR source and build directions

The helper is the standalone Cargo workspace in this directory. Its preferred
source form is the checked-in Rust source, `Cargo.toml`, and `Cargo.lock`.
Build the helper with its own manifest and lock; do not add its dependency graph
to `core/Cargo.toml`.

OAR OCR source is pinned in `recipe.toml` to
`https://github.com/GreatV/oar-ocr` revision
`84817f58d7a0932d5368a9fa18105ba5d842af29`.

The helper-private ONNX Runtime 1.31.0 binary is acquired from the exact
per-target wheels in `recipe.toml`. Its corresponding source is
`https://github.com/microsoft/onnxruntime` revision
`402979ac1bef89dcd55add41eaa7277c407599eb`. The Eigen source revision used by
that build is `1d8b82b0740839c0de7f1242a3585e3390ff5f33` from
`https://gitlab.com/libeigen/eigen`. The runtime archive is kept intact in the
acquisition cache for provenance; staging extracts the native runtime and
its shared-provider dependency when present, plus that archive's `LICENSE` and
`ThirdPartyNotices.txt`.

Detector, recognizer, and dictionary source revisions, exact URLs, hashes,
dictionary materialization, CPU settings, and framing limits are pinned in
`recipe.toml`. The helper source is AGPL-3.0-only. OAR and the PP-OCR model and
dictionary retain their separate Apache-2.0 notices and attribution in
`NOTICE` and `LICENSE-APACHE`.

For a native host build, from the repository root:

```sh
cargo build --manifest-path core/screen-ocr-helper/Cargo.toml --release --locked
```

Main-workspace builds continue to use their existing
ONNX Runtime dependency; the helper does not upgrade it.

The pinned runtime's corresponding source includes its build scripts and
instructions at [ONNX Runtime source](https://github.com/microsoft/onnxruntime/tree/402979ac1bef89dcd55add41eaa7277c407599eb).
The exact Eigen source is available at [Eigen source](https://gitlab.com/libeigen/eigen/-/tree/1d8b82b0740839c0de7f1242a3585e3390ff5f33).
Runtime dependency notices are shipped in full, and Rust dependency versions,
checksums, source locations and notices are shipped in
`screen-ocr-rust-sources.json` and `screen-ocr-rust-NOTICES.txt`.
