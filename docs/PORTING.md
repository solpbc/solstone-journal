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

## iOS canary

`check-rust-ios` is a native-macOS, `aarch64-apple-ios` compile canary for
portable Rust libraries. It is engineering insurance for a later mobile-runtime
effort, not a claim that the journal currently supports an iOS runtime. The
desktop-first product decision keeps mobile-runtime requirements out of the
current journal release.

The Makefile's `check-rust-ios` target is the executable authority for what the
canary checks. Its current exclusions are grouped here so a green result is not
mistaken for workspace-wide iOS coverage:

- desktop and journal-host entry points: `solstone-core`,
  `solstone-core-journal-cli`, `solstone-core-sol-link`,
  `solstone-core-generate-wire`, `solstone-core-serving`
- journal-host HTTP and browser surfaces: `solstone-core-convey-http`,
  `solstone-core-convey-shell`, `solstone-core-clients-web`,
  `solstone-core-settings-web`, `solstone-core-facets-web`,
  `solstone-core-convey-body`
- journal-host storage, ingest, import, and rebuild paths:
  `solstone-core-indexer-store`, `solstone-core-indexer-query`,
  `solstone-core-entity`, `solstone-core-facets`, `solstone-core-segment`,
  `solstone-core-ingest`, `solstone-core-entities`,
  `solstone-core-body-rebuild`, `solstone-core-import-host`
- confidential-service components: `solstone-core-spp-attest`,
  `solstone-core-spp-ratls`
- native media and model components: `solstone-core-transcribe`,
  `solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`,
  `solstone-core-describe`, `solstone-core-observe-audio`,
  `solstone-core-vad-analyze`

An exclusion is a boundary of this canary, not evidence that the package fails
to compile for iOS or that it is accepted into a future iOS runtime. Conversely,
the included `solstone-core-speakers` (DSP/clustering) and
`solstone-core-indexer` (markdown discovery) remain portability canaries.

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
| `macos-arm64` | same | `cargo run -p solstone-core-distribution --bin solstone-distribution --release -- produce macos-arm64 <outdir>`, then [`core/distribution/macos.sh`](../core/distribution/macos.sh) (`pkg`, `bootstrap`, `gatekeeper`, `talent`, `speakers` roles) | A Darwin/arm64 host with Xcode and the Developer ID Application/Installer identities plus the notarytool profile named in `inventory.toml`'s `[apple]` table | Installs the signed, notarized `.pkg`; proves Gatekeeper assessment + staple, a fresh-login-shell PATH resolution, and that a real talent and the real speaker models run from the installed tree | Shipped-target artifact (build + install/smoke) |

Two more targets are checked without shipping:

- **Windows** (`x86_64-pc-windows-msvc`) has an `inventory.toml` `[[target]]`
  entry for field-set validation only — no production entry is admitted for
  it yet. `make check-rust-windows` is a Linux-host cfg-seam classification
  against a self-expiring exclusion ledger; it does not compile or link MSVC
  code. `WIN_REMOTE_HOST=... make win-host-ci` builds and tests a named
  subset natively on a real Windows host, but its own explicit not-run list
  excludes packaging, install, signing, and smoke. Both are cross-target
  drift evidence. **No shipped-target evidence exists for Windows** — there
  is no Windows release to produce it.
- **iOS** (`aarch64-apple-ios`) is `check-rust-ios`, a macOS-host compile
  canary over a large exclusion list (§ iOS canary, above) — engineering
  insurance, not a claim of iOS runtime support. Cross-target drift evidence
  only. **No shipped-target evidence exists for iOS** — nothing is packaged,
  installed, or smoke-tested.

## Native dependency proof

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
and the ReFS enumeration/revalidation/archive matrix. ReFS claimed-removal remains
unrun/skipped and unsupported. Do not treat this transport gate as evidence for
Callosum, packaging, installation, signing, or smoke tests.

The opt-in processing bundle is a direct rpath proof. `make build-sandbox-processing` is self-preparing: it invokes `make check-rust-onnx-stage` internally to validate or stage the pinned host ONNX Runtime, then builds the helpers and installs their payload into the effective Cargo target. Run [`make check-rust-sandbox-processing-build`](../Makefile#L513) against that same Cargo target as the separate read-only follow-up; the check proves both helpers [start without loader-path variables (`sandbox_processing_check_uses_only_the_existing_payload_and_clears_loader_paths`)](../core/crates/solstone-core-repository-contracts/tests/repository_make_command_graphs.rs#L811) and [performs no build or repair (`sandbox_processing_check_rejects_invalid_payload_before_helpers`)](../core/crates/solstone-core-repository-contracts/tests/repository_make_command_graphs.rs#L710). For the negative proof, copy a helper to a sibling-less scratch directory, clear both loader-path variables, and confirm the loader fails before a structured request error is emitted.

## Related

- [testing.md](testing.md) — `make ci` / `make ci-full`
- [release-evidence-contract.md](release-evidence-contract.md)
- [CHANNEL_ADAPTERS.md](CHANNEL_ADAPTERS.md)
- [JOURNAL_FILESYSTEM_CONTRACT.md](JOURNAL_FILESYSTEM_CONTRACT.md) — journal root, identity, kind, and refusal vocabulary

[Microsoft Visual C++ runtime components](../core/distribution/windows-msvc-NOTICE.md)
