# catalog and source tools

These standalone build tools prepare localization resources and review source literals. They are outside the application Cargo workspace and install payload. The fixtures are synthetic; the tools do not declare application language support. [Dependency notices](licenses/NOTICES.md).

## catalog compiler

Prepare the compiler's pinned Cargo dependencies during an explicit setup step. Build and run it offline afterward:

```sh
cargo fetch --manifest-path tools/i18n/Cargo.toml --locked
cargo build --manifest-path tools/i18n/Cargo.toml --locked --offline
cargo test --manifest-path tools/i18n/Cargo.toml --locked --offline
cargo clippy --manifest-path tools/i18n/Cargo.toml --locked --offline --all-targets -- -D warnings
cargo fmt --manifest-path tools/i18n/Cargo.toml --check
tools/i18n/target/debug/i18n-catalog compile tools/i18n/fixtures/synth --channel production
tools/i18n/target/debug/i18n-catalog coverage tools/i18n/fixtures/synth --channel production
```

Each catalog contains `catalog.json`, `source/<source_locale>.ftl`, `meta/<dotted-id>.json`, `translations/<locale>.ftl`, `history.json` and `source-lock.json`. The [synthetic catalog](fixtures/synth) is an executable example. [Sidecar schema](schema/metadata.v1.json), [IR schema](schema/ir.v1.json) and compiler admission checks define the format together.

Author message identities as stable dotted IDs. These are compiler input: stock Fluent readers cannot read them directly. Runtime resource keys are the persisted `m` prefix followed by the lowercase UTF-8 hex encoding of the identity. Keep history in version control; retired identities remain tombstones. Argument contracts, namespace owners and protected-token values cannot change under an existing identity. Create a new identity for those changes.

The portable profile admits plain text, named typed arguments, one top-level cardinal select, or an enum select with an explicit default. Functions, terms, message references, nested selects and rich markup refuse admission. Scalar contracts distinguish strings, booleans, integers and cardinal quantities. Integers stay within JavaScript's exact-integer range; cardinal quantities also stay within signed 32-bit range. Numeric style is explicit. `max_length` is a layout hint.

Put commands, flags, accepted input, URLs, mark words and meaningful whitespace in `protected_slots`. Refer to them with the same named-variable syntax as arguments, but keep their names disjoint from runtime argument names. The compiler injects their exact sidecar bytes; callers cannot supply them. Translations preserve each branch's slot multiplicity and may reorder slots. Pseudolocalization changes prose while preserving these slots and runtime argument identities.

Every sidecar names its logical `surfaces` and compiled `bundles` separately. Coverage checks required message × locale × bundle cells, plus branch and slot populations. The source locale uses canonical source directly; another locale requires an explicit current catalog. English fallback cannot satisfy its coverage.

The [locale registry](locales.json) is compiled into this tool. It owns namespaces, bundle locale membership, locale purpose, tier and channel state. Its current entries are synthetic. Production excludes draft, test and pseudo locales even when a contradictory release flag says released. A consuming repository pins the reviewed compiler source and registry bytes together; adding its real namespaces, bundles or supported languages is an explicit integration change.

For a new catalog, bootstrap history explicitly with `history: false` in `catalog.json`, run `history <catalog>`, then set `history: true`. Compiled output requires history; a catalog claiming existing history cannot recreate a missing registry. Use `retire <catalog> --id <id> [--replacement <id>]` to keep a permanent retirement record.

After changing source prose, run `lock <catalog>`. Copy each changed message's compiler-produced `source` hash from `source-lock.json` into its sidecar `source_revision`; do not hash FTL text yourself. Review and update each translation, then run `lock <catalog> --acknowledge <locale>`. A source-only lock refresh preserves old locale acknowledgments, so unchanged translations stay stale. Withdraw obsolete content approvals; a supplied approval binds its exact locale, source revision and translation revision. Payment, legal, statutory and consent entries reject `auto_merge: true`.

`emit <catalog> --channel <channel> --out <file>` writes deterministic validated IR. `pseudo <catalog> --out <absent-directory>` creates en-XA/ar-XB authoring catalogs and reports their coverage denominator. These catalogs go through the compiler before any runtime reader.

`export <catalog> --out <absent-directory>` creates a shared catalog bundle with a deterministic `import-lock.json`. `import <bundle> --dest <absent-directory>` verifies the exact manifest and member digests before publishing a staged tree and retaining the lock. Commit the imported bytes and lock with the consuming repository's source pin. Import verifies transport integrity; run normal catalog compilation afterward to validate semantics and current coverage. Local build and CI routes need no network or localization service. The tools use Rust and Swift, so a consuming repository need not introduce Python.

## target resources

Use `targets` to lower a validated catalog. Repeat `--target` to request more than one format:

```sh
tools/i18n/target/debug/i18n-catalog targets tools/i18n/fixtures/runtime-contract \
  --channel production --target rust --target esm --target apple --target android --target gettext \
  --out /var/tmp/i18n-target-resources
```

The target names are `rust`, `esm`, `apple`, `android`, `webextension` and `gettext`. Each output cell follows the registry's bundle and locale membership. Missing or stale translations fail before publication. An unsupported target contract produces an error; it does not fall back to a different locale, numeric style or argument type. All requested targets are staged together. A target refusal preserves the previous output tree; successful regeneration replaces it, removing locales excluded by the new channel selection. Use an absent destination or a directory previously produced by this command.

Canonical FTL remains compiler input. Use the emitted resource keys and adapters at runtime. Enum variants share the parent semantic identity and use its declared default for an unmatched string value. A missing argument or a value of the wrong scalar type remains an error.

Rust output includes `MessageId`, typed scalar values, and `render`/`render_dotted` entry points. The ESM API exports `render`, `integer` and `cardinal`; pass named argument pairs, preserving the declared scalar types. Strings and booleans keep their ordinary JavaScript types. Displayed runtime arguments retain interpolation isolation. Protected payloads remain literal data.

WebExtension resources are for explicitly eligible static manifest messages. Dynamic UI messages use the Fluent runtime. The ESM output includes its pinned local runtime and license; serve those files with the generated module, without a CDN or bundler.

Native resource compilation and runtime rendering are separate checks. Compile xcstrings with Xcode, link Android resources with AAPT2 and the platform jar, and check PO files with GNU gettext. Then run the emitted call contract in the adopting target and compare arguments, selected branches, protected tokens and meaningful whitespace. A parser written alongside the generator does not replace its platform reader.

The output's `cases.json` supplies synthetic inputs and a structured `call_contract` with the selected resource key and ordered native argument types. `own_reader_call` is an internal parser description, not executable platform code. `logical_expected` records scalar text; `expected` is null when native number formatting or ESM rendering still needs an independent golden. Its platform-reader status remains pending until that reader runs. Preserve the native call's argument order and display types, including the original numeric plural quantity and any separate display string. Literal percent signs are escaped for one native formatting pass, including messages with no runtime arguments.

Rust and gettext refuse locale-formatted numbers. ESM uses native Intl; Apple uses Foundation; Android supplies an ICU number-format helper. The pinned Rust plural reader supports the initial English profile; Arabic quantities require declared bounds within -100..100 because its larger-value rules differ from current native readers. Unsupported plural-rule families refuse.

Android writes the catalog's source locale into `values/` when that locale is selected for the bundle and channel. Otherwise it uses the selected locale that sorts first by tag. Each selected locale also gets its own qualified table. Coverage still requires every declared cell; the default table cannot satisfy a missing translation. Private-use tags require an explicit native projection. WebExtension output likewise requires explicit native locale folders.

InfoPlist/AppShortcuts catalogs, desktop/AppStream, winget and store-listing publication are outside these generators. Test locale declarations and generated fixtures must stay out of application packaging.

The committed [runtime contract fixture](fixtures/runtime-contract) includes independent exact Rust and ESM goldens for signed quantities, integer bounds, reordered arguments, enum defaults, protected tokens and whitespace. After the example generation above, run its ESM cases under Node:

```sh
node tools/i18n/verify-esm.mjs /var/tmp/i18n-target-resources \
  tools/i18n/fixtures/runtime-contract/expected-esm.json
```

Run process-based generated API checks explicitly; routine `make ci` keeps compiler tests in process:

```sh
cargo test --manifest-path tools/i18n/Cargo.toml --locked --offline --features full-tests
```

## source review

The [extractor](extract/README.md) uses Rust `syn` and an installed SwiftSyntax parser. Prepare dependencies explicitly, then run its locked, offline checks:

```sh
make -C tools/i18n/extract prepare
make -C tools/i18n/extract ci
make -C tools/i18n/extract verify
cargo build --manifest-path tools/i18n/extract/Cargo.toml --locked --offline
```

Use the `.` discovery root to cover new top-level source directories. A narrower directory root covers new files only within that directory. On a Mac, build the Swift helper and pass its absolute path when the tree contains Swift:

```sh
make -C tools/i18n/extract swift-parser
make -C tools/i18n/extract verify-swift
tools/i18n/extract/target/debug/i18n-extract discover "$PWD" /var/tmp/inventory.json . \
  --swift-parser "$PWD/tools/i18n/extract/swift-discover"
```

Every discovered occurrence needs an explicit classification and reason. Presentation occurrences also name a stable dotted message ID. Keep the complete inventory with those decisions and run `lint` against the current source. It refuses new files, changed files and unreviewed literals, including strings returned by helpers or nested in formatting macros. Reviewers still decide which expressions are presentation and how typed message calls should replace them.

Use `reconcile` to compare presentation IDs with the catalog's ID set. Generate a plan from reviewed replacement expressions, then apply it explicitly. A stale file or byte span refuses the entire plan before source writes. Publication is not a crash-atomic transaction across files; use an exclusive source checkout during apply. Command timings are written separately from deterministic artifacts.

## locale tests

The [locale harnesses](locale/README.md) reject a run unless its requested locale canary succeeds. On glibc, the runner builds private locales and starts the command with the verified environment. Rust standard formatting ignores that environment; check the localization adapter's rendered output separately.

Apple tests must call the canary inside the actual test process and declare test localizations in the test target. The disposable Foundation check covers the harness; it does not qualify simulator or SwiftUI rendering. Test declarations and fixtures must stay out of application packaging.

## shared push catalog

The journal owns [`i18n/push`](../../i18n/push), a small shared message set for phone notifications and synthetic rendering tests. `en-US` supplies English. `en-GB` is a labelled test locale, excluded by production channel selection even though its release flag is set. It is not a supported product language.

Export the catalog with `export`, then import that bundle into each client with `import`. Commit the retained import lock and generated native resources with the client. Pin the compiler revision and registry with the bundle. Generate production resources with `--channel production`; generate test resources separately with `--channel development`. Test resources belong only in local test artifacts and never in a store upload.
