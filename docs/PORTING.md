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

Subsystem logic stays in `check-rust-ios` unless a host-only adapter makes
that impossible.

Excluded on purpose (product shape, not deferred debt):

- `solstone-core-indexer-store` / `solstone-core-indexer-query` — bundled C SQLite
- `solstone-core-speakers-analyze` / `solstone-core-speakers-onnx` — ONNX host runtime
- `solstone-core-sol-link` — desktop/link surface; phones use `spl-swift`
- `solstone-core-convey-http` / `solstone-core-settings-web` — journal-host HTTP; phones are clients

`solstone-core-speakers` (DSP/clustering) and `solstone-core-indexer` (markdown
discovery) stay in the canary.

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
