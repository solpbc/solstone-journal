# BK-1 handoff: remaining `solstone/` readers

BK-1 removed every **cargo compile-time** input under `solstone/` (80 include/path sites + 9 `build.rs` reads). The Python tree stays. This inventory is what the deletion lode inherits.

The **`runtime-read` list is larger than before this lode.** BK-1 converted several compile-time `include_*` sites into runtime `fs::read` so `cargo build` / `clippy --all-targets` no longer need those files present. Those new readers are marked **added**.

Categories:

- `runtime-read` — Rust opens the path at run time (including tests).
- `swift-build` — Swift/`make` only; not a cargo input.
- `copy-source` — originals whose crate copies the byte-identity gate covers.
- `python-only` — remainder of `solstone/`; Rust no longer opens these paths.

Do not treat `~/.config/solstone/…` or `solstone/journal` identity strings as journal-tree readers.

## `runtime-read` — added by BK-1

| Path | Site |
|---|---|
| `solstone/think/native/import/authority.toml` | `solstone-core-import-host/tests/cli_journal_source.rs` |
| `solstone/apps/speakers/native/authority.toml` | `solstone-core-convey-shell/tests/speakers_cli_routes.rs` |
| `solstone/apps/entities/native/authority.toml` | `solstone-core-entities/src/router_tests.rs` |
| `solstone/apps/facets/native/authority.toml` | `solstone-core-entities/src/router_tests.rs` |
| `solstone/apps/home/workspace.html` | `solstone-core-home-web/src/assets.rs` test |
| `solstone/apps/home/static/home.js` | `solstone-core-home-web/src/assets.rs` test |
| `solstone/think/services/spp_attest/ratls/ratls-contract.json` | `solstone-core-spp-ratls/src/ratls/contract.rs` test |
| `solstone/talent/journal/contract/bundle.json` | `solstone-core/src/contract/bundle.rs` test (this file was already a runtime reader elsewhere; the **include_bytes!** became a runtime read) |

## `runtime-read` — pre-existing

Production / library path joins (not exhaustive of comments):

| Path / prefix | Site |
|---|---|
| `solstone/talent`, `solstone/apps` | `solstone-core/src/main.rs`, `solstone-core-cortex/src/{service,process}.rs`, `solstone-core-thinking/src/generators.rs`, `solstone-core-think-cli`, `solstone-core-talent-cli/src/lib.rs`, `solstone-core-convey-shell/src/thinking_sol_reads.rs` |
| `solstone/think/templates` | cortex process + convey-shell thinking reads |
| `solstone/talent/chat.md`, `solstone/talent/read.md` | `solstone-core-talent-cli/src/show.rs` |
| `core/crates/solstone-core/src/contract/schemas/*.schema.json` (`REQUIRED_SOURCES`) | `solstone-core/src/contract/bundle.rs` |
| `solstone/think/contract/layout.json` | contract builder |
| `solstone/talent/journal/contract/bundle.json` | `solstone-core/tests/contract_process_isolation.rs` |
| `solstone/observe/transcribe/parakeet_helper` | `solstone-core-transcribe` + `coreml_install.rs` `Package.swift` |
| `solstone/observe/_silero_vad.py` | vad-analyze / transcribe differential tests |
| `solstone/observe/transcribe/_fixtures/parakeet_sample.wav` | transcribe tests |
| `solstone/think/journal_default.json` | `solstone-core-journal-config` tests |
| `solstone/think/importers/{apple_health,oura,health_dedupe,sync,oura_auth}.py` | body restore tests |
| `solstone/think/providers/parakeet_install.py` | local install tests |
| `solstone/apps/thinking/{workspace.html,static/thinking.js}` | convey-shell thinking_runs_contracts |
| `solstone/__init__.py` | contract path discovery |

Many other test files mention `solstone/…` as Python oracles (callosum registry conformance, differentials). They are runtime/test readers, not compile inputs.

## `swift-build`

`solstone/observe/transcribe/parakeet_helper/` source and `_bin` stay together. Not a cargo input. Python-cut lode owns this.

## `copy-source` (byte-identity gate)

107 files (resolved bytes):

- `solstone/convey/static/**` — 77 files
- `solstone/convey/templates/init.html`
- `solstone/think/link/mark_assets/{glyphs,colors,words}.json`
- `solstone/observe/{describe,extract}.md` and `{describe,extract}.schema.json`
- `solstone/observe/categories/{browsing,calendar,code,gaming,media,meeting,messaging,productivity,reading,social,terminal}.md`
- `solstone/observe/categories/{calendar,meeting,messaging}.schema.json`
- `solstone/apps/settings/install_copy.py`
- `solstone/apps/chat/copy.py`
- `solstone/convey/sol_initiated/copy.py`
- `solstone/apps/backup/copy.py`
- `solstone/think/activities.py`
- `solstone/apps/network/copy.py`
- `solstone/think/services/outcomes.py`
- `solstone/think/pairing/config.py`

`vendor/VENDOR.md` is a symlink in `solstone/`; the crate copy is a regular file with identical target bytes.

## `python-only`

Everything else under `solstone/` that is not in the three lists above. Rust compile no longer opens those paths. Flask, pytest, talents, and scripts still do.

Notable: 23 `authority.toml` files stay under `solstone/**/native/` (architecture rule). They are runtime-read by the BK-1 test rewrites above and by the Python inventory generator; they are not cargo build inputs.
