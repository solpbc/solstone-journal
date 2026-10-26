# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
SHELL := /bin/bash
BUILD_ROOT ?= /var/tmp/journal-ios-runtime-build
CACHE_DIR ?= /var/tmp/journal-mobile-cache
JOBS ?= 6
DEVICE ?=
DESTINATION ?= generic/platform=iOS
PROJECT_SPEC ?= project.yml
FINALIZE_ARGS ?=
BUILD_NUMBER ?=
RUST_LIB := $(BUILD_ROOT)/rust/aarch64-apple-ios/debug/libsolstone_ios_runtime.a
APPRAISAL_LIB := $(BUILD_ROOT)/appraisal-rust/aarch64-apple-ios/debug/libsolstone_ios_appraisal_helper.a
APPRAISAL_NATIVE_WORK := $(BUILD_ROOT)/appraisal-native-work
APPRAISAL_NATIVE_DIR := $(BUILD_ROOT)/appraisal-native
APPRAISAL_RESOURCES := $(BUILD_ROOT)/appraisal-resources
APPRAISAL_BUILD_RECORD := $(BUILD_ROOT)/appraisal-native-build.json
NVAT_LIB := $(APPRAISAL_NATIVE_DIR)/libnvat-all.a
BACKUP_NATIVE_WORK ?= $(BUILD_ROOT)/backup-native-work
BACKUP_LIB := $(BUILD_ROOT)/backup-native/librestic_bridge.a
BACKUP_RESOURCES := Resources/BackupRuntime
APP := $(BUILD_ROOT)/derived/Build/Products/Debug-iphoneos/JournalRuntimeDev.app
NATIVE_FEATURES := $(if $(AUDIO_MODELS_DIR),audio) $(if $(filter project-owner.yml,$(PROJECT_SPEC)),owner-app)
.PHONY: build rust appraisal-rust appraisal-native backup-native payload project finalize install run check
rust:
	IPHONEOS_DEPLOYMENT_TARGET=26.0 $(if $(RFDETR_LIB_DIR),RFDETR_LIB_DIR='$(RFDETR_LIB_DIR)') $(if $(PARAKEET_LIB_DIR),PARAKEET_LIB_DIR='$(PARAKEET_LIB_DIR)') cargo build --locked --manifest-path native/Cargo.toml --features '$(NATIVE_FEATURES)' --target aarch64-apple-ios --target-dir $(BUILD_ROOT)/rust
appraisal-rust:
	IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo build --locked --manifest-path appraisal-helper/Cargo.toml --target aarch64-apple-ios --target-dir $(BUILD_ROOT)/appraisal-rust
appraisal-native:
	python3 build_gpu_appraisal.py --work-dir $(APPRAISAL_NATIVE_WORK) --cache-dir $(CACHE_DIR) --output-lib $(NVAT_LIB) --resource-dir $(APPRAISAL_RESOURCES) --build-record $(APPRAISAL_BUILD_RECORD) --jobs $(JOBS)
backup-native:
	python3 build_backup.py --work-dir $(BACKUP_NATIVE_WORK) --cache-dir $(CACHE_DIR) --output-lib $(BACKUP_LIB) --resource-dir $(BACKUP_RESOURCES) --jobs $(JOBS)
payload:
	AUDIO_MODELS_DIR='$(AUDIO_MODELS_DIR)' python3 package_payload.py
project: payload backup-native
	xcodegen generate --spec $(PROJECT_SPEC)
build: rust appraisal-rust appraisal-native project
	xcodebuild -project JournalRuntimeDev.xcodeproj -scheme JournalRuntimeDev -configuration Debug -destination '$(DESTINATION)' -derivedDataPath $(BUILD_ROOT)/derived -allowProvisioningUpdates RUST_LIB=$(RUST_LIB) APPRAISAL_LIB=$(APPRAISAL_LIB) NVAT_LIB=$(NVAT_LIB) BACKUP_LIB=$(BACKUP_LIB) $(if $(BUILD_NUMBER),CURRENT_PROJECT_VERSION=$(BUILD_NUMBER),) build
	python3 finalize.py $(BUILD_ROOT) --nvattest-lib $(NVAT_LIB) --appraisal-build-record $(APPRAISAL_BUILD_RECORD) --appraisal-resources $(APPRAISAL_RESOURCES) --backup-lib $(BACKUP_LIB) --backup-work $(BACKUP_NATIVE_WORK) $(FINALIZE_ARGS)
.PHONY: build-owner install-owner
build-owner:
	@test -n "$(AUDIO_MODELS_DIR)" || (echo "Set AUDIO_MODELS_DIR to the verified mobile audio resources"; exit 2)
	$(MAKE) build PROJECT_SPEC=project-owner.yml FINALIZE_ARGS='--bundle-identifier app.solstone.journal.owner.dev'
install-owner: build-owner
	@test -n "$(DEVICE)" || (echo "Set DEVICE to a connected device identifier"; exit 2)
	xcrun devicectl device install app --device $(DEVICE) $(APP)
install: build
	@test -n "$(DEVICE)" || (echo "Set DEVICE to a connected device identifier"; exit 2)
	xcrun devicectl device install app --device $(DEVICE) $(APP)
run:
	@test -n "$(DEVICE)" || (echo "Set DEVICE to a connected device identifier"; exit 2)
	xcrun devicectl device process launch --device $(DEVICE) --console --terminate-existing app.solstone.journal.runtime.dev $(ARGS)
check:
	cargo fmt --manifest-path native/Cargo.toml -- --check
	cargo clippy --locked --manifest-path native/Cargo.toml --all-targets --no-deps -- -D warnings
	cargo test --locked --manifest-path native/Cargo.toml --features full-tests
	cargo test --locked --manifest-path ../../core/Cargo.toml -p solstone-core-spp-attest --features test-hooks ios_appraisal::tests
	cargo test --locked --manifest-path ../../core/Cargo.toml -p solstone-core-spp-attest --features test-hooks,full-tests ios_service_tools::tests
	cargo test --locked --manifest-path ../../core/Cargo.toml -p solstone-core-spp-attest --features test-hooks,full-tests,ios-owner-app ios_service_tools::tests
	cargo fmt --manifest-path appraisal-helper/Cargo.toml -- --check
	cargo clippy --locked --manifest-path appraisal-helper/Cargo.toml --all-targets --no-deps -- -D warnings
	cargo test --locked --manifest-path appraisal-helper/Cargo.toml
	python3 test_gpu_appraisal_receipt.py
	python3 test_serving_receipts.py
	python3 ../mobile-audio/test_prepare.py
