# Rust workspace

The conversion is closed. There is no Python reference to port from. This
file is the workspace rules that still bind.

The architectural map (plates, strands, cables) lives in
[`conversion/`](conversion/README.md).

## Scope

The workspace is `core/`. Edition 2024, `rust-version = "1.95"`,
`license = "AGPL-3.0-only"`, inherited from `core/Cargo.toml`. Every `.rs`
file starts with the two-line SPDX header in `AGENTS.md`.

Do not add shims, fallback aliases, or dual Python/Rust paths.

## iOS cross-build

`check-rust-ios` compiles and links the whole workspace for an iOS device
(`aarch64-apple-ios`) on a macOS host with Xcode, with default features plus
`solstone-core/journal-mcp-endpoint`. There are no package exclusions. This is
compilation support. Nothing is packaged, installed or run, and it is not a
claim that the journal runs on iOS: process lifecycle, supervisor admission and
readiness refuse on iOS in source, and every engine is still a separate program.
Installation identity still maps iOS to the macOS platform tag and an absolute
installation path. That is inherited, not a qualified iOS identity design.

- **Minimum iOS:** 26.0, set as `IPHONEOS_DEPLOYMENT_TARGET` for cargo's
  children only. Exported into a shell that also builds host tools, Apple clang
  would target iOS for those as well; the vendored FFmpeg build removes it from
  its own configure and make environment for that reason.
- **ONNX Runtime:** the upstream 1.25.0 iOS package (static, with the CoreML
  provider), pinned by digest in `scripts/ios_cross_build.sh`. `make
  ci-full-prep-ios` (part of `ci-full-prep`) acquires it into `core/ios-build`;
  the gate itself only verifies it (`ios-onnx-runtime` prerequisite).
- **Rust objects:** `core/target-ios`, separate from desktop CI.

```sh
make ci-full-prep-ios check-rust-ios
```

The simulator target is not built: FFmpeg bindings are committed for the
device target only.

## Target evidence

The shipped target list and the per-target binary/lane split are canonical in
[`core/distribution/inventory.toml`](../core/distribution/inventory.toml)
(`[[target]]` for the target list, `[[entry]]` for the binary/lane closure).
Re-derive from that file rather than this table if the two ever disagree.

| Target | Target triple(s) | Build command | Required host / toolchain | Evidence produced | Evidence class |
|---|---|---|---|---|---|
| `linux-x86_64` | `x86_64-unknown-linux-musl` (statically-linked binaries) and `x86_64-unknown-linux-gnu` via the pinned zig cross-linker (dynamically-linked binaries, the `zig-gnu-2.27` lane) | `make ci` | An x86_64 Linux host with the pinned `rust-toolchain.toml` toolchain | Formatting, Clippy, the selected library/binary test closure, offline license/bans/sources policy | Host gate — native `gnu` build only; does not exercise the shipped `musl` lane |
| `linux-x86_64` | same | `make check-rust-distribution` (`cargo run -p solstone-core-distribution --bin solstone-distribution -- produce linux-x86_64 <outdir>`, with `AR_<triple>`/`RANLIB_<triple>` bound to the pinned zig wrappers), then `make check-rust-distribution-cleanroom` and `make check-systemd-test` | The same x86_64 Linux host, plus both `x86_64-unknown-linux-{musl,gnu}` Rust targets, a pinned zig cross-linker, and Podman/Docker | Real cross-built `.tar.gz`/`.deb`/`.rpm` with a poison log proving no toolchain fallthrough, then a zero-Python install+launch proof in disposable containers and a live `systemd --user` install/upgrade/crossover proof against the produced `.deb`/`.rpm` | Shipped-target artifact (build + install/smoke) |
| `linux-aarch64` | `aarch64-unknown-linux-musl` and `aarch64-unknown-linux-gnu` (same lane split) | `make check-rust-distribution` (`produce linux-aarch64 <outdir>`), cross-compiled from the same x86_64 host — no native aarch64 host is needed to build | The same x86_64 Linux host as `linux-x86_64`, plus the `aarch64-unknown-linux-{musl,gnu}` Rust targets | Real cross-built `.tar.gz`/`.deb`/`.rpm` with the same poison proof | Shipped-target artifact — build half only. **Absent:** `cleanroom.sh` and `check-systemd-test` run disposable containers on the build host's own architecture and have no aarch64 lane, so this repository's automated tooling does not install or smoke-test the produced aarch64 artifacts; that needs a real aarch64 Linux host outside this repo |
| `macos-arm64` | `aarch64-apple-darwin` (every binary — macOS has one native lane, not the Linux musl/gnu split) | `check-rust-macos` (part of `make ci-full`) | A Darwin/arm64 host with the pinned Rust toolchain | Full-workspace compile check (`cargo test --workspace --all-targets --no-run`), including every classified `full-tests` feature closure | Host gate — compile-only, no run and no package |
| `macos-arm64` | same | `cargo run -p solstone-core-distribution --bin solstone-distribution --release -- produce macos-arm64 <outdir>`, then [`core/distribution/macos.sh`](../core/distribution/macos.sh) (`tar`, `gatekeeper`, `talent`, `speakers` roles) | A Darwin/arm64 host with Xcode and the Developer ID Application identity plus the notarytool profile named in `inventory.toml`'s `[apple]` table | Extracts the `.tar.gz` of signed, notarized binaries that Journal.app embeds; proves every executable is notarized and starts, every loaded payload is signed and notarized, and that a real talent and the real speaker models run from the extracted tree | Shipped-target artifact (build + install/smoke) |

**Windows** (`x86_64-pc-windows-msvc`, target `windows-x86_64`) ships from
2.0.21 as a per-user installer. Its payload entries are admitted in
`inventory.toml` on the `msvc-native` lane and are produced, signed and packaged
on a Windows host. `make check-rust-windows` remains a Linux-host cfg-seam
classification against a self-expiring exclusion ledger, and
`WIN_REMOTE_HOST=... make win-host-ci` builds and tests a named subset natively;
both are cross-target drift evidence. The shipped-target evidence is the signed
installer's install pass on a clean Windows machine as a standard user.

One more target is checked without shipping:

- **iOS** (`aarch64-apple-ios`) is `check-rust-ios`, a macOS-host build and
  link of the whole workspace (§ iOS cross-build, above). **No shipped-target
  evidence exists for iOS** — nothing is packaged, installed, or smoke-tested.

## Native dependency proof

[Android cross-building](ANDROID_CROSS_BUILD.md) is an opt-in compilation/link
check for the full default-feature workspace and the MCP feature. It does not
qualify an Android runtime or add a shipped target.

A crate that adds C/C++ build steps or native linkage is not done after
`cargo test`. Prove it still builds on every shipped target in the table
above — the musl/gnu lane split on both Linux targets and the native lane on
macOS. Toolchain and linker behavior belongs in checked-in release paths, not
a local shell profile.

The first native Windows substrate gate is available to configured operators
as `WIN_REMOTE_HOST=user@host SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT=account make win-host-ci`.
It transfers an exact, source-bound Git snapshot, verifies the workspace lockfile
digest on the Windows checkout, runs the ordinary-owner journal inventory control
through an interactive limited-token scheduled task, and can opt into Cloud Files
and the ReFS enumeration/revalidation/archive matrix. It also carries its own
FFmpeg build toolchain: the build host has MSVC but deliberately no ambient
MSYS2 shell, GNU make, NASM or libclang, so the gate runs `solstone-distribution
acquire ffmpeg-windows-tools` on the host and stages the four pinned archives
under the same pins the controlled producer uses. ReFS claimed-removal remains
unrun/skipped and unsupported. Do not treat this transport gate as evidence for
Callosum, packaging, installation, signing, or smoke tests.

The opt-in processing bundle is a direct rpath proof. `make build-sandbox-processing` is self-preparing: it invokes `make check-rust-onnx-stage` internally to validate or stage the pinned host ONNX Runtime, then builds the helpers and installs their payload into the effective Cargo target. Run [`make check-rust-sandbox-processing-build`](../Makefile#L513) against that same Cargo target as the separate read-only follow-up; the check proves both helpers [start without loader-path variables (`sandbox_processing_check_uses_only_the_existing_payload_and_clears_loader_paths`)](../core/crates/solstone-core-repository-contracts/tests/repository_make_command_graphs.rs#L811) and [performs no build or repair (`sandbox_processing_check_rejects_invalid_payload_before_helpers`)](../core/crates/solstone-core-repository-contracts/tests/repository_make_command_graphs.rs#L710). For the negative proof, copy a helper to a sibling-less scratch directory, clear both loader-path variables, and confirm the loader fails before a structured request error is emitted.

## Related

- [testing.md](testing.md) — `make ci` / `make ci-full`
- [release-evidence-contract.md](release-evidence-contract.md)
- [JOURNAL_FILESYSTEM_CONTRACT.md](JOURNAL_FILESYSTEM_CONTRACT.md) — journal root, identity, kind, and refusal vocabulary

[Microsoft Visual C++ runtime components](../core/distribution/windows-msvc-NOTICE.md)

[Microsoft Edge WebView2 loader](../core/distribution/windows-webview2-loader-NOTICE.md)
