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

## Native dependency proof

A crate that adds C/C++ build steps or native linkage is not done after
`cargo test`. Prove the supported release targets still build: Linux x86_64
musl, Linux aarch64 musl, and macOS arm64. Toolchain and linker behavior
belongs in checked-in release paths, not a local shell profile.

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
