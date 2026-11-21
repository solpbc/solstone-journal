# The journal app for mac

`journal.app` is how the journal runs on a mac. It is a Swift app around the journal this repository builds: it sets the journal up, starts it and keeps it running, opens your journal in a window, and updates itself. Everything that writes your journal is built from [`core/`](../../core); this app never writes what your journal holds.

Most people should download the app from <https://solstone.app/download/journal>.

## Build from source

You need a mac running macOS 15 or later with Apple silicon, and the full Xcode app with Swift 6.1 or later. Open Xcode once and finish its first-run setup before building. `make icons` also needs `brew install librsvg`.

```bash
cd platforms/macos
make build            # debug build of the app, its watchdog helper and the icon generator
make ci               # the unit gate: brand assets, Swift tests, packaging tests
make journal-app-dev  # a local journal.app with a runtime built from this checkout
make run-journal      # build that app, open it and stream its logs
```

From the repository root, `make macos-app-ci`, `make macos-app-build` and `make macos-app-dev` run the same targets. The root `make ci` never builds the mac app, so Linux and Windows work needs no Xcode.

## How it fits together

- **This package:** `SolstoneJournalMac` ([Package.swift](Package.swift)). Its targets are `journal` (the app), `JournalRuntime` (sets up, starts, supervises and stops the native runtime), `JournalWatchdogMain` (the `solstone-watchdog` helper inside the app), `journal-icon-gen`, and the test-only `journal-integration-fixture`.
- **Shared mac code** comes from [solstone-macos](https://github.com/solpbc/solstone-macos), the repository of the solstone app for mac, at one exact commit pinned in `Package.swift` and `Package.resolved`: core types (`SolstoneCore`), the journal mark (`JournalMarkKit`), setup chrome (`SetupChrome`), updates (`UpdateKit`) and the watchdog (`WatchdogKit`). Change the revision and `Package.resolved` together, never to a branch or tag.
- **App text** comes from the [English catalog](i18n/catalog/source/en-US.ftl). On a Mac, `make catalog` runs the repository’s compiler and Xcode’s string catalog tool. SwiftPM builds from the committed resources. Both app assembly targets copy the app and library bundles into `Contents/Resources/`.
- **The runtime inside the app** is a released native archive, embedded by its SHA-256 and verified against its signed manifest (`make journal-native-runtime-accepted`). `make journal-native-runtime` builds one from this checkout for development only. The same repository does not mean the same commit: an app-only release can embed a runtime published earlier.
- **Release helpers** shared with the solstone app (signing keychain, appcast publishing, GitHub releases, the CI lock) run from a checkout of solstone-macos at the pinned commit. `MACOS_SUPPORT_ROOT` names it; by default it is SwiftPM's own copy under `.build/checkouts/`, and `make support-check` refuses any other revision or a dirty tree.

## Tests

Three tiers, and they do not substitute for each other:

| Tier | Command | What it proves |
|---|---|---|
| Unit gate | `make ci` | App and runtime-adapter logic against fakes, the committed AX contract and icon, and the packaging scripts. Runs under the host-wide CI lock with a throwaway test keychain. |
| Native integration | `make integration-native` | The app's real runtime code against a native journal in an isolated home: setup, required models, start, readiness, health and a clean stop. Opt-in; its first line names the runtime that ran. |
| Release acceptance | per release | The signed, notarized app installed fresh and updated from the previous release, used end to end with the solstone app. |

`make integration-fixture` builds the test-only executable that the solstone app's own integration tier starts to get a real journal under this app's supervisor.

## What stays the same

The app's identity did not change when its source moved into this repository: bundle ID `app.solstone.journal`, update feed `https://updates.solstone.app/journal-macos/appcast.xml` and its signing key, the `app.solstone.journal.watchdog` helper, macOS 15 or later on Apple silicon, and the folder you chose for your journal. Releases are tagged `journal-v<version>-build-<build>`. Releases up to 2.0.44 (build 83) were built from solstone-macos and keep their tags there; [MIGRATION.md](MIGRATION.md) has the path map.

## Logging

```bash
log stream --predicate 'subsystem == "app.solstone.journal" OR subsystem == "app.solstone.journal.watchdog"'
```

## License

AGPL-3.0-only. Copyright (c) 2026 sol pbc. Third-party notices for what the app bundles are in the repository's [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md#the-journal-app-for-mac).
