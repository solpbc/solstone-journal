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

The opt-in processing bundle is a direct rpath proof. After `make check-rust-onnx-stage`, run `make build-sandbox-processing` and then `make check-rust-sandbox-processing-build` against the same Cargo target; the check proves both helpers start without loader-path variables and performs no build or repair. For the negative proof, copy a helper to a sibling-less scratch directory, clear both loader-path variables, and confirm the loader fails before a structured request error is emitted.

## Related

- [testing.md](testing.md) — `make ci` / `make ci-full`
- [release-evidence-contract.md](release-evidence-contract.md)
- [CHANNEL_ADAPTERS.md](CHANNEL_ADAPTERS.md)
