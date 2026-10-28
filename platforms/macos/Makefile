# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# The Journal app for macOS: build, test, package, sign and publish journal.app.
# Run from this directory (platforms/macos) or through the repository root's
# macos-app-* delegates. Nothing here is part of the root `make ci`: Linux and
# Windows work never needs Swift, Xcode or an Apple keychain.
#
# Two roots matter and they are not the same: this package root (Swift sources,
# resources, .build, the app and DMG) and the repository root two levels up
# (the native journal runtime's source and its Git identity).
#
# Shared Mac code and release helpers come from solstone-macos at the exact
# commit pinned in Package.swift and Package.resolved. SwiftPM materializes that
# commit under .build/checkouts/solstone-macos; release hosts pass an explicit
# clean checkout at the same commit as MACOS_SUPPORT_ROOT. `make support-check`
# refuses any other revision.

.PHONY: build resolve support-check test ci ci-release ax-contract snapshot integration-native integration-fixture \
        icons check-icons-deps check-brand-assets-fresh release-universal-journal journal-native-runtime \
        journal-native-fetch-accepted journal-native-runtime-accepted journal-app-dev assemble-journal-app \
        journal-app-unsigned bundle-dist-journal run-journal dmg-journal notarize-journal staple-journal \
        verify-notarization-journal release-dmg-journal release-dmg-smoke-journal supply-chain-check \
        bump-release-journal unlock-signing signing-check release-preflight publish-preflight \
        publish-appcast-journal publish-appcast-journal-staging github-release-journal \
        ja1r-gate-sync ja1r-gate-clean-tree verify-ja1r-gate-journal clean

.DEFAULT_GOAL := build

REPO_ROOT              := $(abspath ../..)
MACOS_SUPPORT_ROOT     ?= $(CURDIR)/.build/checkouts/solstone-macos
SUPPORT_SCRIPTS         = $(MACOS_SUPPORT_ROOT)/scripts

# Distribution signing (Apple Developer ID + notarization). The signing keychain
# and notary profile are host state shared with the capture app; unlocking and
# checking them is the support checkout's single implementation.
DEVELOPER_ID_APP       ?= Developer ID Application: sol pbc (7QCG8V4M6H)
NOTARY_PROFILE         ?= sol-pbc-notary
SIGNING_KEYCHAIN       ?= $(HOME)/Library/Keychains/sol-signing.keychain-db
RELEASE_IDENTITY        = python3 $(SUPPORT_SCRIPTS)/release_identity.py
JOURNAL_DIST_VERSION   := $(shell /usr/bin/env python3 -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["CFBundleShortVersionString"])' Sources/journal/Info.plist 2>/dev/null || echo 0.0.0)
JOURNAL_DIST_BUILD     := $(shell /usr/bin/env python3 -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["CFBundleVersion"])' Sources/journal/Info.plist 2>/dev/null || echo 0)
JOURNAL_DMG_NAME       ?= $(shell $(RELEASE_IDENTITY) identity --app journal --version '$(JOURNAL_DIST_VERSION)' --build '$(JOURNAL_DIST_BUILD)' --field dmg_name 2>/dev/null)
SPARKLE_ARTIFACT_DIR   ?= $(MACOS_SUPPORT_ROOT)/vendor/sparkle
SPARKLE_FRAMEWORK      ?= $(SPARKLE_ARTIFACT_DIR)/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework
# The DMG window art is shared with the capture app's image.
DMG_BACKGROUND         ?= $(MACOS_SUPPORT_ROOT)/assets/dmg-background@2x.png
# Two --arch flags are load-bearing: they select SwiftPM's Apple/Xcode backend,
# which writes to .build/apple/Products/Release (what bundle-dist-journal copies
# from). Journal is arm64-only, so both flags are arm64; do not collapse them to
# one flag, and do not swap in --build-system swiftbuild, which writes to
# .build/out/Products/Release instead.
JOURNAL_RELEASE_APPLE_ARCH_FLAGS := --arch arm64 --arch arm64

# journal.app carries the Rust-native distribution tree directly. Development
# builds bind a clean source commit of this repository; shipping builds consume
# an already-accepted archive by SHA-256. Both embed a provenance receipt beside
# the entrypoints. The same repository does not mean the same commit: a
# wrapper-only cut embeds a previously accepted payload.
JOURNAL_NATIVE_SOURCE_DIR ?= $(REPO_ROOT)
JOURNAL_NATIVE_EXPECTED_COMMIT ?=
JOURNAL_NATIVE_TARGET ?= macos-arm64
JOURNAL_NATIVE_OUTPUT_DIR ?= .build/journal-native-output
JOURNAL_NATIVE_RUNTIME_DIR ?= .build/journal-native-runtime
# The released runtime every owner runs, embedded in the installed journal app.
INSTALLED_JOURNAL_RUNTIME ?= /Applications/journal.app/Contents/Resources/solstone-runtime
JOURNAL_NATIVE_PROVENANCE_RECEIPT ?= $(JOURNAL_NATIVE_RUNTIME_DIR)/journal-native-provenance.json
JOURNAL_NATIVE_ACCEPTED_ARCHIVE ?=
JOURNAL_NATIVE_ACCEPTED_SHA256 ?=
JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE ?=
JOURNAL_NATIVE_ACCEPTED_MANIFEST ?=
JOURNAL_NATIVE_ACCEPTED_MANIFEST_SIGNATURE ?=
JOURNAL_NATIVE_ACCEPTED_RELEASE_RECEIPT ?=
JOURNAL_NATIVE_ACCEPTED_SIGNING_RECEIPT ?=
JOURNAL_NATIVE_MINISIGN ?= /opt/homebrew/bin/minisign
# Or name the release and let the compose fetch its five signed inputs from the
# published origin: JOURNAL_NATIVE_ACCEPTED_VERSION=<version> with the accepted
# JOURNAL_NATIVE_ACCEPTED_SHA256 replaces the five paths above. The manifest is
# trusted only once its minisign signature verifies against the pinned release
# key, and each other input only if it matches the signed manifest's digest.
JOURNAL_NATIVE_ACCEPTED_VERSION ?=
JOURNAL_NATIVE_ORIGIN ?= https://updates.solstone.app/solstone-journal
JOURNAL_NATIVE_FETCH_DIR ?= .build/journal-native-accepted
ifneq ($(strip $(JOURNAL_NATIVE_ACCEPTED_VERSION)),)
ifneq ($(strip $(JOURNAL_NATIVE_ACCEPTED_ARCHIVE)$(JOURNAL_NATIVE_ACCEPTED_MANIFEST)$(JOURNAL_NATIVE_ACCEPTED_MANIFEST_SIGNATURE)$(JOURNAL_NATIVE_ACCEPTED_RELEASE_RECEIPT)$(JOURNAL_NATIVE_ACCEPTED_SIGNING_RECEIPT)),)
$(error JOURNAL_NATIVE_ACCEPTED_VERSION fetches the signed journal inputs; do not also pass their paths)
endif
JOURNAL_NATIVE_ACCEPTED_STEM := $(JOURNAL_NATIVE_FETCH_DIR)/solstone-journal-$(JOURNAL_NATIVE_ACCEPTED_VERSION)-$(JOURNAL_NATIVE_TARGET)
JOURNAL_NATIVE_ACCEPTED_ARCHIVE := $(JOURNAL_NATIVE_ACCEPTED_STEM).tar.gz
JOURNAL_NATIVE_ACCEPTED_MANIFEST := $(JOURNAL_NATIVE_ACCEPTED_STEM).manifest.json
JOURNAL_NATIVE_ACCEPTED_MANIFEST_SIGNATURE := $(JOURNAL_NATIVE_ACCEPTED_STEM).manifest.json.minisig
JOURNAL_NATIVE_ACCEPTED_RELEASE_RECEIPT := $(JOURNAL_NATIVE_ACCEPTED_STEM).release
JOURNAL_NATIVE_ACCEPTED_SIGNING_RECEIPT := $(JOURNAL_NATIVE_ACCEPTED_STEM).signing.json
endif
JOURNAL_NATIVE_RUNTIME_TREE_MAP ?= .build/journal-native-runtime-tree.map
JOURNAL_NATIVE_RUNTIME_ENTRY_PROVENANCE := journal.app/Contents/Resources/SolstoneJournalMac_journal.bundle/Contents/Resources/Resources/runtime-entry-candidate-provenance.json

# Build debug version
build:
	swift build

# Resolve the pinned dependencies; this is what materializes the support checkout.
resolve:
	swift package resolve

# The support checkout must be a clean Git checkout at exactly the revision
# Package.resolved pins for solstone-macos. A release never runs helpers from
# another revision, a dirty tree or a checkout SwiftPM merely happened to leave.
support-check:
	@PINNED="$$(python3 -c 'import json,sys; pins=[p for p in json.load(open(sys.argv[1]))["pins"] if p["identity"]=="solstone-macos"]; print(pins[0]["state"]["revision"] if len(pins)==1 else "")' Package.resolved)"; \
		test -n "$$PINNED" || { echo "error: Package.resolved pins no solstone-macos revision"; exit 1; }; \
		ACTUAL="$$(git -C '$(MACOS_SUPPORT_ROOT)' rev-parse HEAD 2>/dev/null)"; \
		test "$$ACTUAL" = "$$PINNED" || { echo "error: MACOS_SUPPORT_ROOT=$(MACOS_SUPPORT_ROOT) is at '$$ACTUAL', not the pinned $$PINNED (run 'make resolve', or pass a clean checkout of solstone-macos at the pin)"; exit 1; }; \
		test -z "$$(git -C '$(MACOS_SUPPORT_ROOT)' status --porcelain)" || { echo "error: MACOS_SUPPORT_ROOT=$(MACOS_SUPPORT_ROOT) is dirty"; exit 1; }; \
		echo "support: solstone-macos $$PINNED at $(MACOS_SUPPORT_ROOT)"
	@python3 $(SUPPORT_SCRIPTS)/sparkle_dependency.py verify

# Run tests
test:
	swift test

# The unit gate: brand freshness, `swift test --no-parallel` and this package's
# Python suite, under the host-wide CI lock and an ephemeral test keychain. The
# lock and keychain sequence are the support checkout's run-ci.sh, shared with
# the capture app because both mutate the same per-user keychain slot.
ci: resolve support-check
	@$(SUPPORT_SCRIPTS)/run-ci.sh --package-root "$(CURDIR)"

ci-release: resolve support-check
	@$(SUPPORT_SCRIPTS)/run-ci.sh --package-root "$(CURDIR)" -c release -Xswiftc -DSOLSTONE_TEST_SUPPORT

# Regenerate journal-ax-contract.json from the Swift source of truth.
ax-contract: resolve support-check
	AX_CONTRACT_REGEN=1 $(SUPPORT_SCRIPTS)/run-ci.sh --package-root "$(CURDIR)" --filter JournalAXContractTests.contractMatchesCommittedFileOrRegenerates

# Render view snapshots
snapshot:
	swift test --filter Snapshot

# On-demand integration tier: the Journal app's real runtime code against a
# native journal runtime, inside an isolated home. Opt-in only; `make ci`
# compiles these suites and lists them as skipped. Runtime selection, in order:
# SOLSTONE_NATIVE_RUNTIME_DIR, the staged accepted runtime (a release clone has
# one after `make journal-native-runtime-accepted ...`), then the runtime inside
# the installed journal app. The first output line names what actually ran: the
# version and a digest over the whole runtime tree (the launcher binaries are
# identical across releases, so a single file's digest would not tell them apart).
# Not a release gate: signed-artifact, Sparkle and relay acceptance stay on the
# release rig.
integration-native:
	@RUNTIME="$${SOLSTONE_NATIVE_RUNTIME_DIR:-}"; \
		if [ -z "$$RUNTIME" ]; then \
			if [ -x "$(JOURNAL_NATIVE_RUNTIME_DIR)/bin/solstone" ]; then RUNTIME="$(JOURNAL_NATIVE_RUNTIME_DIR)"; \
			elif [ -x "$(INSTALLED_JOURNAL_RUNTIME)/bin/solstone" ]; then RUNTIME="$(INSTALLED_JOURNAL_RUNTIME)"; \
			else echo "error: no native journal runtime: stage one, install journal.app, or set SOLSTONE_NATIVE_RUNTIME_DIR"; exit 1; fi; \
		fi; \
		test -x "$$RUNTIME/bin/solstone" || { echo "error: no solstone binary at $$RUNTIME/bin/solstone"; exit 1; }; \
		echo "integration-native: runtime $$("$$RUNTIME/bin/solstone" journal --version) tree-sha256=$$(cd "$$RUNTIME" && find bin lib share -type f | sort | xargs shasum -a 256 | shasum -a 256 | cut -d' ' -f1) at $$RUNTIME"; \
		SOLSTONE_NATIVE_INTEGRATION=1 SOLSTONE_NATIVE_RUNTIME_DIR="$$RUNTIME" swift test --no-parallel --filter NativeIntegration

# The test-only fixture the capture app's door test starts (solstone-macos,
# `make integration-native SOLSTONE_JOURNAL_INTEGRATION_FIXTURE=<path>`). A
# debug build: it reaches the runtime's containment reader through @testable.
# Never embedded in an app or published.
integration-fixture:
	@swift build --product journal-integration-fixture
	@echo "$$(swift build --show-bin-path)/journal-integration-fixture"


release-universal-journal:
	swift build -c release $(JOURNAL_RELEASE_APPLE_ARCH_FLAGS) --product journal
	swift build -c release $(JOURNAL_RELEASE_APPLE_ARCH_FLAGS) --product solstone-watchdog


journal-native-runtime:
	@python3 scripts/journal_native_provenance.py check-source \
		--source-dir "$(abspath $(JOURNAL_NATIVE_SOURCE_DIR))" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)"
	@$(MAKE) -C "$(abspath $(JOURNAL_NATIVE_SOURCE_DIR))" ci-full-prep
	@python3 scripts/journal_native_provenance.py check-source \
		--source-dir "$(abspath $(JOURNAL_NATIVE_SOURCE_DIR))" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)"
	@rm -rf "$(JOURNAL_NATIVE_OUTPUT_DIR)" "$(JOURNAL_NATIVE_RUNTIME_DIR)"
	@mkdir -p "$(JOURNAL_NATIVE_OUTPUT_DIR)" "$(JOURNAL_NATIVE_RUNTIME_DIR)"
	@(cd "$(abspath $(JOURNAL_NATIVE_SOURCE_DIR))" && cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked --offline -- produce "$(JOURNAL_NATIVE_TARGET)" "$(abspath $(JOURNAL_NATIVE_OUTPUT_DIR))")
	@set -- "$(JOURNAL_NATIVE_OUTPUT_DIR)"/*.tar.gz; \
		test "$$#" -eq 1 && test -f "$$1" || { echo "error: expected exactly one native journal tarball in $(JOURNAL_NATIVE_OUTPUT_DIR)"; exit 1; }; \
		tar -xzf "$$1" -C "$(JOURNAL_NATIVE_RUNTIME_DIR)"
	@test -x "$(JOURNAL_NATIVE_RUNTIME_DIR)/bin/solstone" || { echo "error: native journal runtime is missing bin/solstone"; exit 1; }
	@python3 scripts/journal_native_provenance.py write-receipt \
		--source-dir "$(abspath $(JOURNAL_NATIVE_SOURCE_DIR))" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)" \
		--target "$(JOURNAL_NATIVE_TARGET)" \
		--output-dir "$(abspath $(JOURNAL_NATIVE_OUTPUT_DIR))" \
		--runtime-dir "$(abspath $(JOURNAL_NATIVE_RUNTIME_DIR))" \
		--receipt "$(abspath $(JOURNAL_NATIVE_PROVENANCE_RECEIPT))"

journal-native-fetch-accepted:
	@if [ -n "$(strip $(JOURNAL_NATIVE_ACCEPTED_VERSION))" ]; then \
		test -n "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" || { echo "error: JOURNAL_NATIVE_ACCEPTED_SHA256=<sha256> required"; exit 1; }; \
		python3 scripts/journal_native_provenance.py fetch-accepted \
			--version "$(JOURNAL_NATIVE_ACCEPTED_VERSION)" \
			--target "$(JOURNAL_NATIVE_TARGET)" \
			--expected-sha256 "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" \
			--origin "$(JOURNAL_NATIVE_ORIGIN)" \
			--minisign "$(JOURNAL_NATIVE_MINISIGN)" \
			--output-dir "$(abspath $(JOURNAL_NATIVE_FETCH_DIR))"; \
	fi

journal-native-runtime-accepted: journal-native-fetch-accepted
	@test -n "$(JOURNAL_NATIVE_ACCEPTED_ARCHIVE)" || { echo "error: JOURNAL_NATIVE_ACCEPTED_ARCHIVE=/absolute/path required"; exit 1; }
	@test -n "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" || { echo "error: JOURNAL_NATIVE_ACCEPTED_SHA256=<sha256> required"; exit 1; }
	@test -n "$(JOURNAL_NATIVE_EXPECTED_COMMIT)" || { echo "error: JOURNAL_NATIVE_EXPECTED_COMMIT=<accepted-sha40> required"; exit 1; }
	@test -n "$(JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE)" || { echo "error: JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE=<receipt-or-record> required"; exit 1; }
	@python3 scripts/journal_native_provenance.py stage-accepted \
		--archive "$(abspath $(JOURNAL_NATIVE_ACCEPTED_ARCHIVE))" \
		--expected-sha256 "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)" \
		--acceptance-evidence "$(JOURNAL_NATIVE_ACCEPTANCE_EVIDENCE)" \
		--target "$(JOURNAL_NATIVE_TARGET)" \
		--workspace-root "$(CURDIR)" \
		--runtime-dir "$(abspath $(JOURNAL_NATIVE_RUNTIME_DIR))" \
		--receipt "$(abspath $(JOURNAL_NATIVE_PROVENANCE_RECEIPT))"

# Development app: a source-built runtime and an unsigned debug-shaped bundle.
journal-app-dev: journal-native-runtime
	@echo "Building journal dev app..."
	@swift build -c release --product journal
	@swift build -c release --product solstone-watchdog
	@BUILD_DIR="$$(swift build -c release --show-bin-path)"; \
		echo "Assembling journal.app from $$BUILD_DIR"; \
		rm -rf journal.app; \
		mkdir -p journal.app/Contents/MacOS journal.app/Contents/Resources journal.app/Contents/Frameworks journal.app/Contents/Library/LaunchAgents; \
		cp "$$BUILD_DIR/journal" journal.app/Contents/MacOS/; \
		cp "$$BUILD_DIR/solstone-watchdog" journal.app/Contents/MacOS/; \
		cp Sources/journal/Info.plist journal.app/Contents/; \
		cp Sources/journal/Resources/AppIcon.icns journal.app/Contents/Resources/; \
		cp -R "$(JOURNAL_NATIVE_RUNTIME_DIR)" journal.app/Contents/Resources/solstone-runtime; \
		cp Sources/journal/app.solstone.journal.watchdog.plist journal.app/Contents/Library/LaunchAgents/; \
		test -d "$$BUILD_DIR/solstone_JournalMarkKit.bundle" || { echo "error: solstone_JournalMarkKit.bundle missing from $$BUILD_DIR"; exit 1; }; \
		cp -R "$$BUILD_DIR/solstone_JournalMarkKit.bundle" journal.app/Contents/Resources/; \
		test -d "$$BUILD_DIR/solstone_SetupChrome.bundle" || { echo "error: solstone_SetupChrome.bundle missing from $$BUILD_DIR"; exit 1; }; \
		cp -R "$$BUILD_DIR/solstone_SetupChrome.bundle" journal.app/Contents/Resources/; \
		if [ -d "$$BUILD_DIR/SolstoneJournalMac_journal.bundle" ]; then \
			cp -R "$$BUILD_DIR/SolstoneJournalMac_journal.bundle" journal.app/Contents/Resources/; \
		fi; \
		if [ -d "$(SPARKLE_FRAMEWORK)" ]; then \
			cp -R "$(SPARKLE_FRAMEWORK)" journal.app/Contents/Frameworks/; \
		elif [ -d "$$BUILD_DIR/Sparkle.framework" ]; then \
			cp -R "$$BUILD_DIR/Sparkle.framework" journal.app/Contents/Frameworks/; \
		else \
			echo "error: Sparkle.framework not found at $(SPARKLE_FRAMEWORK) or $$BUILD_DIR/Sparkle.framework"; \
			exit 1; \
		fi; \
		RPATH_LOG="$$(mktemp -t journal-rpath)"; \
		if install_name_tool -add_rpath "@executable_path/../Frameworks" journal.app/Contents/MacOS/journal 2>"$$RPATH_LOG"; then \
			rm -f "$$RPATH_LOG"; \
		elif grep -Eq 'would duplicate path|already exists' "$$RPATH_LOG"; then \
			rm -f "$$RPATH_LOG"; \
		else \
			cat "$$RPATH_LOG"; \
			rm -f "$$RPATH_LOG"; \
			exit 1; \
		fi; \
		echo "✓ Assembled: journal.app"; \
		find journal.app/Contents -maxdepth 3 \( -path '*/Versions' -o -path '*/_CodeSignature' \) -prune -o \( -type f -o -type d \) -print | sort

assemble-journal-app: release-universal-journal
	@rm -rf journal.app
	@mkdir -p journal.app/Contents/MacOS journal.app/Contents/Resources journal.app/Contents/Frameworks journal.app/Contents/Library/LaunchAgents
	@cp .build/apple/Products/Release/journal journal.app/Contents/MacOS/
	@cp .build/apple/Products/Release/solstone-watchdog journal.app/Contents/MacOS/
	@cp Sources/journal/Info.plist journal.app/Contents/
	@cp Sources/journal/Resources/AppIcon.icns journal.app/Contents/Resources/
	@cp Sources/journal/app.solstone.journal.watchdog.plist journal.app/Contents/Library/LaunchAgents/
	@test -d .build/apple/Products/Release/solstone_JournalMarkKit.bundle || { echo "error: solstone_JournalMarkKit.bundle missing from .build/apple/Products/Release"; exit 1; }
	@test -d .build/apple/Products/Release/solstone_SetupChrome.bundle || { echo "error: solstone_SetupChrome.bundle missing from .build/apple/Products/Release"; exit 1; }
	@test -d .build/apple/Products/Release/SolstoneJournalMac_journal.bundle || { echo "error: SolstoneJournalMac_journal.bundle missing from .build/apple/Products/Release"; exit 1; }
	@cp -r .build/apple/Products/Release/solstone_JournalMarkKit.bundle journal.app/Contents/Resources/
	@cp -r .build/apple/Products/Release/solstone_SetupChrome.bundle journal.app/Contents/Resources/
	@cp -r .build/apple/Products/Release/SolstoneJournalMac_journal.bundle journal.app/Contents/Resources/

journal-app-unsigned: assemble-journal-app

bundle-dist-journal: unlock-signing signing-check journal-native-runtime-accepted release-universal-journal assemble-journal-app
	@cp -R "$(JOURNAL_NATIVE_RUNTIME_DIR)" journal.app/Contents/Resources/solstone-runtime
	@python3 scripts/journal_native_provenance.py write-candidate \
		--archive "$(abspath $(JOURNAL_NATIVE_ACCEPTED_ARCHIVE))" \
		--expected-sha256 "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)" \
		--target "$(JOURNAL_NATIVE_TARGET)" \
		--manifest "$(abspath $(JOURNAL_NATIVE_ACCEPTED_MANIFEST))" \
		--manifest-signature "$(abspath $(JOURNAL_NATIVE_ACCEPTED_MANIFEST_SIGNATURE))" \
		--release-receipt "$(abspath $(JOURNAL_NATIVE_ACCEPTED_RELEASE_RECEIPT))" \
		--signing-receipt "$(abspath $(JOURNAL_NATIVE_ACCEPTED_SIGNING_RECEIPT))" \
		--minisign "$(JOURNAL_NATIVE_MINISIGN)" \
		--runtime-dir "$(abspath journal.app/Contents/Resources/solstone-runtime)" \
		--native-receipt "$(abspath journal.app/Contents/Resources/solstone-runtime/journal-native-provenance.json)" \
		--info-plist "$(abspath journal.app/Contents/Info.plist)" \
		--output "$(abspath $(JOURNAL_NATIVE_RUNTIME_ENTRY_PROVENANCE))" \
		--tree-map "$(abspath $(JOURNAL_NATIVE_RUNTIME_TREE_MAP))"
	@cp -R "$(SPARKLE_FRAMEWORK)" journal.app/Contents/Frameworks/
	@RPATH_LOG="$$(mktemp -t journal-rpath)"; \
		if install_name_tool -add_rpath "@executable_path/../Frameworks" journal.app/Contents/MacOS/journal 2>"$$RPATH_LOG"; then \
			rm -f "$$RPATH_LOG"; \
		elif grep -Eq 'would duplicate path|already exists' "$$RPATH_LOG"; then \
			rm -f "$$RPATH_LOG"; \
		else \
			cat "$$RPATH_LOG"; \
			rm -f "$$RPATH_LOG"; \
			exit 1; \
		fi
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Installer.xpc
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Downloader.xpc
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Frameworks/Sparkle.framework
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Resources/solstone_JournalMarkKit.bundle
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Resources/solstone_SetupChrome.bundle
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/Resources/SolstoneJournalMac_journal.bundle
	@codesign --force --options runtime --timestamp \
		--identifier app.solstone.journal.watchdog \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		journal.app/Contents/MacOS/solstone-watchdog
	@codesign --force --options runtime --timestamp \
		--sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" \
		--entitlements Sources/journal/entitlements.plist \
		journal.app
	@codesign --verify --strict --deep --verbose=2 journal.app
	@test ! -f journal.app/Contents/embedded.provisionprofile || { echo "error: journal.app must not embed a provisioning profile"; exit 1; }
	@! codesign -d --entitlements - --xml journal.app 2>/dev/null | plutil -p - 2>/dev/null | grep -q 'keychain-access-groups' || { echo "error: journal.app must not carry keychain-access-groups"; exit 1; }
	@codesign -dvvv journal.app/Contents/MacOS/solstone-watchdog 2>&1 | grep -Fq 'Identifier=app.solstone.journal.watchdog' || { echo "error: journal watchdog identifier mismatch"; exit 1; }
	@codesign --verify --strict --verbose=2 journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-journal
	@codesign --verify --strict --verbose=2 journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-sol
	@python3 scripts/journal_native_provenance.py verify-candidate \
		--archive "$(abspath $(JOURNAL_NATIVE_ACCEPTED_ARCHIVE))" \
		--expected-sha256 "$(JOURNAL_NATIVE_ACCEPTED_SHA256)" \
		--expected-commit "$(JOURNAL_NATIVE_EXPECTED_COMMIT)" \
		--target "$(JOURNAL_NATIVE_TARGET)" \
		--manifest "$(abspath $(JOURNAL_NATIVE_ACCEPTED_MANIFEST))" \
		--manifest-signature "$(abspath $(JOURNAL_NATIVE_ACCEPTED_MANIFEST_SIGNATURE))" \
		--release-receipt "$(abspath $(JOURNAL_NATIVE_ACCEPTED_RELEASE_RECEIPT))" \
		--signing-receipt "$(abspath $(JOURNAL_NATIVE_ACCEPTED_SIGNING_RECEIPT))" \
		--minisign "$(JOURNAL_NATIVE_MINISIGN)" \
		--runtime-dir "$(abspath journal.app/Contents/Resources/solstone-runtime)" \
		--native-receipt "$(abspath journal.app/Contents/Resources/solstone-runtime/journal-native-provenance.json)" \
		--info-plist "$(abspath journal.app/Contents/Info.plist)" \
		--output "$(abspath $(JOURNAL_NATIVE_RUNTIME_ENTRY_PROVENANCE))" \
		--tree-map "$(abspath $(JOURNAL_NATIVE_RUNTIME_TREE_MAP))"
	@echo "✓ Signed: journal.app (native journal runtime, no profile/keychain-group verified)"

run-journal: journal-app-dev
	@open journal.app
	@/usr/bin/log stream --predicate 'subsystem == "app.solstone.journal" OR subsystem == "app.solstone.journal.watchdog"' --level debug

# Create and sign the DMG with create-dmg (brew install create-dmg). create-dmg
# builds an unsigned DMG; it is signed with --timestamp after, because
# notarization needs the secure timestamp and create-dmg doesn't pass extra
# codesign flags through.
dmg-journal:
	@test -d journal.app || { echo "error: journal.app not found — run make bundle-dist-journal first"; exit 1; }
	@test -f "$(DMG_BACKGROUND)" || { echo "error: $(DMG_BACKGROUND) missing — run make resolve, or pass MACOS_SUPPORT_ROOT"; exit 1; }
	@rm -f $(JOURNAL_DMG_NAME)
	@create-dmg \
	  --volname "journal" \
	  --background "$(DMG_BACKGROUND)" \
	  --window-pos 200 200 \
	  --window-size 640 400 \
	  --icon-size 128 \
	  --icon "journal.app" 180 200 \
	  --app-drop-link 460 200 \
	  --hide-extension "journal.app" \
	  --no-internet-enable \
	  $(JOURNAL_DMG_NAME) journal.app
	@codesign --force --timestamp --sign "$(DEVELOPER_ID_APP)" --keychain "$(SIGNING_KEYCHAIN)" $(JOURNAL_DMG_NAME)
	@echo "✓ Built: $(JOURNAL_DMG_NAME)"

# Submit for notarization and block until Apple responds.
notarize-journal:
	@test -f "$(JOURNAL_DMG_NAME)" || { echo "error: $(JOURNAL_DMG_NAME) not found — run make dmg-journal first"; exit 1; }
	@echo "Submitting $(JOURNAL_DMG_NAME) for notarization (typically 1-5 min)…"
	@xcrun notarytool submit $(JOURNAL_DMG_NAME) \
		--keychain-profile "$(NOTARY_PROFILE)" --keychain "$(SIGNING_KEYCHAIN)" \
		--wait

# Attach the notarization ticket to the DMG so Gatekeeper works offline.
staple-journal:
	@test -f "$(JOURNAL_DMG_NAME)" || { echo "error: $(JOURNAL_DMG_NAME) not found — run make notarize-journal first"; exit 1; }
	@xcrun stapler staple $(JOURNAL_DMG_NAME)


verify-notarization-journal:
	@test -f "$(JOURNAL_DMG_NAME)" || { echo "error: $(JOURNAL_DMG_NAME) not found — run make staple-journal first"; exit 1; }
	@test -d journal.app || { echo "error: journal.app not found — run make bundle-dist-journal first"; exit 1; }
	@spctl --assess --type open --context context:primary-signature -v $(JOURNAL_DMG_NAME) || \
		{ echo "journal spctl verification failed"; exit 1; }
	@xcrun stapler validate $(JOURNAL_DMG_NAME)
	@codesign --verify --strict --verbose=2 journal.app/Contents/MacOS/solstone-watchdog || { echo "journal watchdog signature invalid"; exit 1; }
	@codesign -dvvv journal.app/Contents/MacOS/solstone-watchdog 2>&1 | grep -q 'TeamIdentifier=7QCG8V4M6H' || { echo "journal watchdog missing Team ID 7QCG8V4M6H"; exit 1; }
	@codesign -dvvv journal.app/Contents/MacOS/solstone-watchdog 2>&1 | grep -q 'flags=0x10000(runtime)' || { echo "journal watchdog missing hardened runtime flag"; exit 1; }
	@codesign -dvvv journal.app/Contents/MacOS/solstone-watchdog 2>&1 | grep -q 'Identifier=app.solstone.journal.watchdog' || { echo "journal watchdog identifier mismatch"; exit 1; }
	@lipo -archs journal.app/Contents/MacOS/solstone-watchdog | grep -q 'arm64' || { echo "journal watchdog missing arm64 slice"; exit 1; }
	@test -x journal.app/Contents/Resources/solstone-runtime/bin/solstone || { echo "error: journal native runtime missing bin/solstone"; exit 1; }
	@codesign --verify --strict --verbose=2 journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-journal
	@codesign --verify --strict --verbose=2 journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-sol
	@echo "✓ $(JOURNAL_DMG_NAME) notarized + stapled"

release-dmg-journal:
	@$(MAKE) bundle-dist-journal
	@$(MAKE) dmg-journal
	@$(MAKE) notarize-journal
	@$(MAKE) staple-journal
	@$(MAKE) verify-notarization-journal
	@echo ""
	@echo "✅ Journal distribution DMG ready: $(JOURNAL_DMG_NAME)"
	@echo "   Signed:  $(DEVELOPER_ID_APP)"
	@echo "   Notary:  $(NOTARY_PROFILE)"
	@echo "   Size:    $$(du -h $(JOURNAL_DMG_NAME) | cut -f1)"

release-dmg-smoke-journal:
	@test -f "$(JOURNAL_DMG_NAME)" || { echo "error: $(JOURNAL_DMG_NAME) not found — run make release-dmg-journal first"; exit 1; }
	@MOUNT="$$(mktemp -d -t journal-smoke)"; \
	    trap "hdiutil detach \"$$MOUNT\" -quiet -force >/dev/null 2>&1 || true; rm -rf \"$$MOUNT\"" EXIT; \
	    hdiutil attach "$(JOURNAL_DMG_NAME)" -mountpoint "$$MOUNT" -nobrowse -quiet || { echo "error: journal hdiutil attach failed"; exit 1; }; \
	    codesign --verify --strict --deep --verbose=2 "$$MOUNT/journal.app" || { echo "error: journal dmg .app codesign verify failed"; exit 1; }; \
	    test -x "$$MOUNT/journal.app/Contents/Resources/solstone-runtime/bin/solstone" || { echo "error: journal native runtime missing bin/solstone"; exit 1; }; \
	    UNREADABLE="$$(find "$$MOUNT/journal.app" \( -type d ! -perm -005 \) -o \( -type f ! -perm -004 \))"; \
	    test -z "$$UNREADABLE" || { echo "error: journal.app has entries other accounts on the Mac cannot read:"; echo "$$UNREADABLE"; exit 1; }; \
	    "$$MOUNT/journal.app/Contents/Resources/solstone-runtime/bin/solstone" journal --version; \
	    "$$MOUNT/journal.app/Contents/Resources/solstone-runtime/bin/solstone" --version; \
	    echo "release-dmg-smoke-journal: ok"

supply-chain-check:
	@echo "── supply chain checklist ──"
	@test -f "$(REPO_ROOT)/THIRD_PARTY_NOTICES.md" || { echo "error: THIRD_PARTY_NOTICES.md missing"; exit 1; }
	@grep -qiE '^##[[:space:]]+Sparkle' "$(REPO_ROOT)/THIRD_PARTY_NOTICES.md" || { echo "error: THIRD_PARTY_NOTICES.md missing the Mac app's Sparkle entry"; exit 1; }
	@grep -qiE '^##[[:space:]]+Comfortaa' "$(REPO_ROOT)/THIRD_PARTY_NOTICES.md" || { echo "error: THIRD_PARTY_NOTICES.md missing the Mac app's Comfortaa entry"; exit 1; }
	@echo "── native journal runtime ──"
	@if [ -x journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-journal ]; then \
	    codesign -dvvv journal.app/Contents/Resources/solstone-runtime/bin/solstone-core-journal 2>&1 || true; \
	else \
	    echo "(not built yet — run make bundle-dist-journal to assemble the signed native runtime)"; \
	fi
	@echo "supply-chain checklist: ok"


# The sol-signing keychain and notary profile are shared host state; the support
# checkout's targets are their single implementation.
unlock-signing: support-check
	@$(MAKE) -s -C "$(MACOS_SUPPORT_ROOT)" unlock-signing

signing-check: support-check
	@$(MAKE) -s -C "$(MACOS_SUPPORT_ROOT)" signing-check


release-preflight: signing-check
	@if ! git diff --quiet || ! git diff --cached --quiet; then \
		echo "error: working tree has uncommitted changes — commit or stash before release"; \
		git status --short; \
		exit 1; \
	fi
	@# Match each process's program and first argument (a script run by its interpreter), not its
	@# whole argument text: an unrelated process whose arguments merely mention these names is not one.
	@STALE="$$(ps -Ao pid=,args= | awk '{a=$$2; b=$$3; sub(/.*\//,"",a); sub(/.*\//,"",b); if (a ~ /^(tmux-run|notarytool|codesign|wheel-macos)$$/ || a ~ /sign-and/ || b ~ /^(tmux-run|notarytool|codesign|wheel-macos)$$/ || b ~ /sign-and/) print}' || true)"; \
		[ -z "$$STALE" ] || { \
			echo "error: stale signing/build processes — one may be holding a keychain prompt:"; \
			echo "$$STALE"; \
			echo "       investigate with tmux capture-pane, then 'pkill -9 -f <pattern>'"; \
			exit 1; \
		}
	@BRANCH="$$(git rev-parse --abbrev-ref HEAD)"; \
		[ "$$BRANCH" = "main" ] || { echo "warn: releasing from non-main branch '$$BRANCH'"; }
	@AHEAD="$$(git rev-list --count @{u}..HEAD 2>/dev/null || echo 0)"; \
		[ "$$AHEAD" = "0" ] || { echo "warn: $$AHEAD local commit(s) ahead of upstream — push before release"; }
	@echo "✓ release pre-flight clean: tree clean, no stale processes, signing ready"

# Bump the Journal app's version and build, and scaffold this build's changelog entry.
bump-release-journal:
	@test -n "$(VERSION)" || { echo "error: VERSION=... required (e.g. VERSION=1.0.1)"; exit 1; }
	@test -n "$(BUILD)"   || { echo "error: BUILD=... required (e.g. BUILD=2)"; exit 1; }
	@test -z "$(SOLSTONE)" || { echo "error: SOLSTONE= is retired; journal release prep uses VERSION and BUILD only"; exit 1; }
	@CURRENT_BUILD="$(JOURNAL_DIST_BUILD)"; \
		python3 -c "import sys; sys.exit(0 if int('$(BUILD)') > int('$$CURRENT_BUILD') else 1)" || \
		{ echo "error: BUILD=$(BUILD) must be strictly greater than current journal build $$CURRENT_BUILD"; exit 1; }
	@/usr/bin/plutil -replace CFBundleShortVersionString -string "$(VERSION)" Sources/journal/Info.plist
	@/usr/bin/plutil -replace CFBundleVersion -string "$(BUILD)" Sources/journal/Info.plist
	@python3 -c 'import json, pathlib, plistlib; p = pathlib.Path("Sources/journal/Resources/runtime-entry-candidate-provenance.json"); data = json.loads(p.read_text()); info = plistlib.loads(pathlib.Path("Sources/journal/Info.plist").read_bytes()); data["target"] = dict(zip(("bundle_identifier", "bundle_short_version", "bundle_version"), (info["CFBundleIdentifier"], info["CFBundleShortVersionString"], info["CFBundleVersion"]))); p.write_text(json.dumps(data, indent=2) + "\n")'
	@echo "✓ journal Info.plist: CFBundleShortVersionString=$(VERSION), CFBundleVersion=$(BUILD)"

# Generate the app icon from assets/icon-journal.svg, which journal-icon-gen
# renders from the shared journal mark. Requires rsvg-convert (brew install
# librsvg) and iconutil. Output files are committed so fresh checkouts build
# cleanly. SOURCE_DATE_EPOCH keeps the output byte-deterministic.
icons: export SOURCE_DATE_EPOCH := 1700000000
icons: check-icons-deps
	@echo "Generating icons from SVG sources..."
	@swift run -c release journal-icon-gen assets/icon-journal.svg
	@mkdir -p Sources/journal/Resources
	@TMPDIR=$$(mktemp -d) && \
	JOURNAL_ICONSET=$$TMPDIR/JournalAppIcon.iconset && \
	mkdir -p $$JOURNAL_ICONSET && \
	echo "  Rendering journal app icon sizes..." && \
	for size in 16 32 64 128 256 512 1024; do \
		rsvg-convert -w $$size -h $$size assets/icon-journal.svg \
			-o $$TMPDIR/journal_icon_$${size}.png; \
		echo "    $${size}x$${size}  (assets/icon-journal.svg)"; \
	done && \
	\
	cp $$TMPDIR/journal_icon_16.png    $$JOURNAL_ICONSET/icon_16x16.png && \
	cp $$TMPDIR/journal_icon_32.png    $$JOURNAL_ICONSET/icon_16x16@2x.png && \
	cp $$TMPDIR/journal_icon_32.png    $$JOURNAL_ICONSET/icon_32x32.png && \
	cp $$TMPDIR/journal_icon_64.png    $$JOURNAL_ICONSET/icon_32x32@2x.png && \
	cp $$TMPDIR/journal_icon_128.png   $$JOURNAL_ICONSET/icon_128x128.png && \
	cp $$TMPDIR/journal_icon_256.png   $$JOURNAL_ICONSET/icon_128x128@2x.png && \
	cp $$TMPDIR/journal_icon_256.png   $$JOURNAL_ICONSET/icon_256x256.png && \
	cp $$TMPDIR/journal_icon_512.png   $$JOURNAL_ICONSET/icon_256x256@2x.png && \
	cp $$TMPDIR/journal_icon_512.png   $$JOURNAL_ICONSET/icon_512x512.png && \
	cp $$TMPDIR/journal_icon_1024.png  $$JOURNAL_ICONSET/icon_512x512@2x.png && \
	\
	iconutil -c icns $$JOURNAL_ICONSET -o Sources/journal/Resources/AppIcon.icns && \
	echo "  ✓ journal AppIcon.icns" && \
	rm -rf $$TMPDIR && \
	echo "Icons generated in Sources/journal/Resources/"

check-icons-deps:
	@which rsvg-convert > /dev/null 2>&1 || \
		(echo "error: rsvg-convert not found — run: brew install librsvg"; exit 1)
	@which iconutil > /dev/null 2>&1 || \
		(echo "error: iconutil not found (requires macOS)"; exit 1)

# Fails when the committed app icon or the generated assets/icon-journal.svg no
# longer matches what `make icons` produces from its source, including the
# shared mark at the pinned support revision. Wired into `make ci`.
check-brand-assets-fresh: check-icons-deps
	@$(SUPPORT_SCRIPTS)/check-brand-assets-fresh.sh Sources/journal/Resources/AppIcon.icns assets/icon-journal.svg


# Cloudflare account id (account "jer") — wrangler whoami must list this.
CF_ACCOUNT_ID          ?= 3f2c1528c7d4d9685819ea9e9e307c92

# Wrangler/CF-auth preflight. Run on the RELEASE HOST *before* dispatching the
# signing-host DMG build — wrangler's session OAuth degrades silently on a ~24h cadence,
# and the R2 publish is the final step, so a stale token otherwise wastes the
# full ~6min build + notarize before failing. `wrangler whoami` exercises the
# same account-lookup path that breaks on degrade; we assert exit 0 AND that the
# account id is listed (a set CLOUDFLARE_API_TOKEN shadowing the OAuth session
# surfaces here too). NOT `/user/tokens/verify` — it reports "Invalid API Token"
# for OAuth tokens even when healthy.
publish-preflight:
	@echo "preflight: checking wrangler Cloudflare auth on the release host…"
	@out="$$(wrangler whoami 2>&1)"; rc=$$?; \
	if [ $$rc -ne 0 ] || ! printf '%s' "$$out" | grep -q '$(CF_ACCOUNT_ID)'; then \
		echo "error: wrangler Cloudflare auth is degraded — run 'wrangler login' (browser OAuth refresh), then retry." >&2; \
		echo "       expected account id $(CF_ACCOUNT_ID) in 'wrangler whoami' output." >&2; \
		exit 1; \
	fi; \
	echo "preflight: wrangler OK (account $(CF_ACCOUNT_ID))"


# publish-appcast.py needs PyNaCl (Sparkle EdDSA enclosure signing) and boto3
# (R2 multipart upload for >300 MiB DMGs). Run it under an ephemeral uv env so
# the publish never depends on the release host's python site-packages.
PUBLISH_PY := uv run --no-project --with 'pynacl>=1.6,<2' --with boto3 python3


# ────────────────────────────────────────────────────────────────
# ja1r linkage gate — production publishes are fail-closed on it.
#
# The linkage gate, rig sync and offline verifier are pinned to one revision.
# The gate's revision file is the only source of that pin;
# the sync never follows the harness repository's main branch.
# The capture app carries its own pin for the sol and paired profiles.
#
# Every lane reads both apps' AX contracts: journal-ax-contract.json from this
# package and ax-contract.json from a solstone-macos checkout, named here as
# SOL_CONTRACT_CHECKOUT (by default the support checkout). Each report and the
# sync receipt bind both commits. The one-command runner is
#   python3 <gate-dir>/release_gate.py --profile journal …
# and the harness README's "One command per profile" is the flag reference.
#
# The journal profile hashes the RELEASED solstone companion DMG the operator
# staged at DMG_NAME; its identity is typed in. The verifier never infers an
# expected identity from the evidence it is checking.
# ────────────────────────────────────────────────────────────────
EXTRO_TOOLS_DIR         ?= $(abspath $(REPO_ROOT)/..)/extro-tools
GATE_TOOLS              := $(EXTRO_TOOLS_DIR)/tools/solstone-macos-gate
SOL_CONTRACT_CHECKOUT   ?= $(MACOS_SUPPORT_ROOT)
JA1R_HOST               ?= ja1r.local
JA1R_HARNESS_DIR        ?= $$HOME/extro-tools/tools/solstone-macos-gate
JA1R_SOL_DIR            ?= $$HOME/projects/solstone-macos
JA1R_JOURNAL_DIR        ?= $$HOME/projects/solstone-journal/platforms/macos
JA1R_GATE_SYNC_RECEIPT  ?= .ja1r-gate/sync-receipt.json
JA1R_GATE_REPORTS_ROOT  ?= .ja1r-gate/reports
JA1R_GATE_REPORT_DIR    ?=
JA1R_GATE_REPORT_DIR_JOURNAL = $(JA1R_GATE_REPORTS_ROOT)/journal-$(JOURNAL_DIST_VERSION)-$(JOURNAL_DIST_BUILD)

PRODUCT_HEAD := $(shell git rev-parse HEAD 2>/dev/null)
SOL_CONTRACT_HEAD = $(shell git -C '$(SOL_CONTRACT_CHECKOUT)' rev-parse HEAD 2>/dev/null)

ja1r-gate-sync:
	@EXTRO_TOOLS_DIR='$(EXTRO_TOOLS_DIR)' JA1R_HOST='$(JA1R_HOST)' \
	 JA1R_HARNESS_DIR='$(JA1R_HARNESS_DIR)' JA1R_SOL_DIR='$(JA1R_SOL_DIR)' \
	 JA1R_JOURNAL_DIR='$(JA1R_JOURNAL_DIR)' JA1R_GATE_SYNC_RECEIPT='$(JA1R_GATE_SYNC_RECEIPT)' \
	 SOL_CHECKOUT='$(SOL_CONTRACT_CHECKOUT)' JOURNAL_CHECKOUT='$(CURDIR)' \
	 PIN_FILE='$(CURDIR)/scripts/ja1r-gate/extro-tools.rev' \
	 bash '$(GATE_TOOLS)/sync.sh'

# Refuse a dirty tree: PRODUCT_HEAD would misrepresent what is being published.
ja1r-gate-clean-tree:
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "error: working tree is dirty — the ja1r evidence is bound to a commit," >&2; \
		echo "       so publishing from a dirty tree would misrepresent what shipped." >&2; \
		exit 1; \
	fi

# $(1) profile, $(2) newline-free list of required var names
define require_gate_vars
	@missing=""; \
	for v in $(2); do \
		eval "val=\$$$$v"; \
		[ -n "$$val" ] || missing="$$missing $$v"; \
	done; \
	if [ -n "$$missing" ]; then \
		echo "error: verify-ja1r-gate-$(1) needs these variables, which are unset:" >&2; \
		for v in $$missing; do echo "         $$v=…" >&2; done; \
		echo "" >&2; \
		echo "       e.g. make verify-ja1r-gate-$(1)$$(for v in $$missing; do printf ' %s=…' "$$v"; done)" >&2; \
		echo "       (the gate never guesses an identity from the reports it is checking)" >&2; \
		exit 1; \
	fi
endef

verify-ja1r-gate-journal: ja1r-gate-clean-tree
	$(call require_gate_vars,journal,JA1R_GATE_SOL_VERSION JA1R_GATE_SOL_BUILD JA1R_GATE_JOURNAL_BASELINE_VERSION JA1R_GATE_JOURNAL_BASELINE_BUILD JA1R_GATE_JOURNAL_RUNTIME_PIN JA1R_GATE_JOURNAL_BASELINE_RUNTIME_PIN DMG_NAME)
	@test -n '$(SOL_CONTRACT_HEAD)' || { echo "error: SOL_CONTRACT_CHECKOUT=$(SOL_CONTRACT_CHECKOUT) is not a solstone-macos git checkout" >&2; exit 1; }
	@test -n '$(JOURNAL_DMG_NAME)' || { echo "error: JOURNAL_DMG_NAME is empty: $(SUPPORT_SCRIPTS)/release_identity.py derives it; set MACOS_SUPPORT_ROOT or JOURNAL_DMG_NAME" >&2; exit 1; }
	python3 '$(GATE_TOOLS)/verify-ja1r-linkage-gate.py' --profile journal \
		--pin-file scripts/ja1r-gate/extro-tools.rev \
		--sol-commit '$(SOL_CONTRACT_HEAD)' --journal-commit '$(PRODUCT_HEAD)' \
		--report-dir '$(or $(JA1R_GATE_REPORT_DIR),$(JA1R_GATE_REPORT_DIR_JOURNAL))' \
		--sync-receipt '$(JA1R_GATE_SYNC_RECEIPT)' \
		--expected-journal-runtime '$(JA1R_GATE_JOURNAL_RUNTIME_PIN)' \
		--expected-journal-baseline-runtime '$(JA1R_GATE_JOURNAL_BASELINE_RUNTIME_PIN)' \
		--sol-dmg '$(DMG_NAME)' \
		--journal-dmg '$(JOURNAL_DMG_NAME)' \
		--sol-target-version '$(JA1R_GATE_SOL_VERSION)' \
		--sol-target-build '$(JA1R_GATE_SOL_BUILD)' \
		--companion-sol-version '$(JA1R_GATE_SOL_VERSION)' \
		--companion-sol-build '$(JA1R_GATE_SOL_BUILD)' \
		--journal-target-version '$(JOURNAL_DIST_VERSION)' \
		--journal-target-build '$(JOURNAL_DIST_BUILD)' \
		--journal-baseline-version '$(JA1R_GATE_JOURNAL_BASELINE_VERSION)' \
		--journal-baseline-build '$(JA1R_GATE_JOURNAL_BASELINE_BUILD)'

# Production publishes are gated. Staging is NOT — it must stay runnable before
# the rig has produced any evidence at all. The appcast and GitHub helpers are
# the support checkout's; they read Sources/journal/Info.plist, the app changelog
# and the DMG from this package root.
publish-appcast-journal: verify-ja1r-gate-journal
	@$(MAKE) publish-preflight
	$(PUBLISH_PY) $(SUPPORT_SCRIPTS)/publish-appcast.py $(JOURNAL_DIST_VERSION) --app journal --build $(JOURNAL_DIST_BUILD)

publish-appcast-journal-staging:
	@$(MAKE) publish-preflight
	$(PUBLISH_PY) $(SUPPORT_SCRIPTS)/publish-appcast.py $(JOURNAL_DIST_VERSION) --app journal --build $(JOURNAL_DIST_BUILD) --staging

# Cut a GitHub Release in solstone-journal: annotated tag
# journal-v<version>-build-<build> + `gh release create` with the DMG attached
# and this build's changelog notes, never marked Latest (the native v<version>
# release keeps that). Run AFTER `make publish-appcast-journal`.
github-release-journal:
	@bash $(SUPPORT_SCRIPTS)/github-release.sh --app journal --app-root "$(CURDIR)" --build $(JOURNAL_DIST_BUILD) $(JOURNAL_DIST_VERSION)

clean:
	swift package clean
	rm -rf .build journal.app
