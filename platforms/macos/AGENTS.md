# The journal app for mac: developer guide

Read this before changing anything in `platforms/macos/`. The repository's root [AGENTS.md](../../AGENTS.md) governs the native runtime; this package is the Swift adapter around it. Start with [README.md](README.md) for the shape of the package.

## Ground rules

- **The app never writes what the journal holds.** Setup, first run (`/init`), and every journal write belong to the native runtime. The Swift side starts, supervises, stops and shows it; on restart it only moves stale runtime state files under `health/` aside.
- **Two roots.** This directory is the Swift package root: sources, resources, `.build`, `journal.app` and DMGs. The repository root (`../..`) is the native runtime's source and the Git identity. Resolve paths from one or the other explicitly. Tests use `journalPackageRoot()` or `#filePath`, never the working directory.
- **Shared code lives in solstone-macos.** `SolstoneCore`, `JournalMarkKit`, `SetupChrome`, `UpdateKit`, `WatchdogKit` and the test-only `MacTestSupport` come from that repository at the exact commit pinned in `Package.swift` and `Package.resolved`. Fix shared behavior there, then move the pin here. Don't copy shared code into this package. Don't depend on the solstone app's own targets.
- **Embedded runtimes are accepted bytes.** Release packaging embeds a published native archive by digest and verifies its signed manifest. Never rebuild a runtime to package a release, and never point the app at a runtime outside its bundle.
- **Installed identity is fixed.** Bundle ID `app.solstone.journal`, the update feed and its signing key, the `solstone-watchdog` helper and its `app.solstone.journal.watchdog` LaunchAgent, Developer ID team `7QCG8V4M6H`, macOS 15 or later, Apple silicon only. A change to any of these is a product decision, not a refactor.
- **Resource bundles.** Both app assembly targets copy the bundles listed in `JOURNAL_RESOURCE_BUNDLES` into `Contents/Resources/`. When a dependency adds resources, update that list and test the assembled app away from the build tree.
- **App text.** Put each new owner-facing message in [the English catalog](i18n/catalog/source/en-US.ftl) with a sidecar under `i18n/catalog/meta/`. Give the whole message one key and name its arguments. Keep wire values and accessibility identifiers independent of display text. When editing English prose, refresh `source-lock.json` with `lock`, copy each changed `source` hash into its sidecar `source_revision`, and do not pass `--acknowledge` for any draft locale, so unchanged draft translations stay stale until re-translated. Run `make catalog` on a Mac to regenerate the Swift accessors, string tables and app metadata. Commit the generated files with the source edit.

## Commands

| Command | Use |
|---|---|
| `make build` | Debug build. |
| `make ci` | The unit gate. It resolves the pinned support checkout, then runs its `run-ci.sh` for this package: brand assets, `swift test --no-parallel`, `scripts/tests`. |
| `make ax-contract` | Regenerate `journal-ax-contract.json` from the Swift source of truth after changing accessibility identifiers, then commit it. |
| `make icons` | Regenerate `assets/icon-journal.svg` and `AppIcon.icns` from the shared mark. |
| `make integration-native` | Opt-in tier against a real native runtime (`SOLSTONE_NATIVE_RUNTIME_DIR`, the staged accepted runtime, or the installed app's). |
| `make journal-app-dev` / `make run-journal` | A local app with a runtime built from this checkout. |
| `make release-dmg-journal JOURNAL_NATIVE_ACCEPTED_VERSION=… JOURNAL_NATIVE_ACCEPTED_SHA256=… JOURNAL_NATIVE_EXPECTED_COMMIT=… JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE=…` | Signed, notarized, stapled DMG on the signing host. |

Run `make ci` before landing a change here. Changes to the runtime adapter (`Sources/JournalRuntime/`) or to the native runtime's setup, supervision or readiness contract also take `make integration-native`.

## Tests

- Swift Testing, in `Tests/journalTests/` (the app) and `Tests/JournalRuntimeTests/` (the runtime adapter). Runtime test helpers live in `Tests/JournalRuntimeTestSupport/`; generic helpers come from `MacTestSupport`.
- Never construct the real app state or touch the real keychain in a headless test. `make ci` runs with a throwaway keychain; a raw `swift test` does not.
- The setup-event parser is held to `Tests/JournalRuntimeTests/Fixtures/native-setup-contract.json`, which `solstone-core-setup` regenerates from its own vocabulary. Don't edit it by hand.
- Test what the code does. Don't add tests that pin Makefile text, copy or design choices.

## Public prose

Everything in this repository is public. Commit messages, comments and docs describe the product and the code. Keep names of internal tools, hosts, people and processes out of them.
