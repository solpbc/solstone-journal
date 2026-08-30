# solstone Makefile
# Rust-native AI-driven desktop journaling toolkit

.PHONY: install uninstall test test-cov test-integration test-performance test-app test-only format format-check install-checks ci ci-full clean clean-install coverage watch versions update update-prices pre-commit skills check-journal-device-sim check-rust-fmt check-rust-msrv check-rust-clippy check-rust-unit check-rust-test check-rust-describe-cli-stubs check-rust-race check-rust-ios check-rust-macos check-rust-windows check-rust-deny check-rust-shipped-binaries build build-sandbox-processing check-rust-sandbox-processing-build check-spl-dependency-pin audit contract check-contract build-native-sol-grammar-oracle check-native-sol-grammar-oracle build-native-sol-root-contract check-native-sol-root-contract build-native-sol-journal-host-commands check-native-sol-journal-host-commands build-journal-access-rejection-inventory check-journal-access-rejection-inventory build-native-sol-inventory check-native-sol-inventory check-native-sol-architecture check-native-sol-coverage check-native-sol-no-python-spawn check-removed-time-parser-ready dev all sandbox sandbox-stop install-models parakeet-helper parakeet-helper-clean check-rust-vad-analyze-test check-rust-onnx-stage check-rust-onnx-test check-rust-pdf-stage check-rust-pdf-test verify service-logs check-api-conventions check-journal-io-access check-journal-io-mechanic check-journal-config-owner check-call-http-only check-channel-adapter-scrub check-brain-health-cutover check-tools-http-only check-local-server-argv-owner check-local-install-transport check-local-generate-cutover check-thinking-cutover check-cogitate-cutover require-win-remote-host sync-win-host win-host-ci brand-sync FORCE

# Default target: build the native workspace.
all: build

# Virtual environment directory
VENV := .venv
VENV_BIN := $(VENV)/bin
VENV_PY := $(VENV_BIN)/python
PYTHON := $(VENV_PY)
RUST_MANIFEST := core/Cargo.toml
GIT ?= git
SCP ?= scp
SSH ?= ssh
WIN_REMOTE_HOST ?=

# Native binaries for the dev and sandbox targets. The wheel used to install
# these into the venv; the distribution tree ships them, and a development
# tree builds them. Run `make build` first if a target here reports one
# missing.
RUST_BIN := core/target/debug
RUST_TARGET_DIR := $(if $(strip $(CARGO_TARGET_DIR)),$(abspath $(CARGO_TARGET_DIR)),$(CURDIR)/core/target)
CI_CARGO_HOME := $(if $(strip $(CARGO_HOME)),$(abspath $(CARGO_HOME)),$(HOME)/.cargo)
CI_RUSTUP_HOME := $(if $(strip $(RUSTUP_HOME)),$(abspath $(RUSTUP_HOME)),$(HOME)/.rustup)
# CI is evidence collection, not an interactive debugger session. Pin these
# settings at the public and internal entry points so direct shells, recursive
# Make, and hostile caller assignments cannot silently recreate incremental or
# workspace debuginfo output. The built-in dev profile name stays unchanged,
# preserving the existing debug/ paths and cross-step reuse.
CI_CARGO_ENV_TARGETS := ci ci-contained ci-under-poison ci-prep-ffmpeg ci-full ci-full-under-poison ci-full-plan ci-full-prep ci-full-prep-cargo ci-full-prep-onnx ci-full-prep-pdf
ifneq ($(strip $(filter $(CI_CARGO_ENV_TARGETS),$(MAKECMDGOALS))),)
export CARGO_INCREMENTAL CARGO_PROFILE_DEV_DEBUG
endif
$(CI_CARGO_ENV_TARGETS): override CARGO_INCREMENTAL := 0
$(CI_CARGO_ENV_TARGETS): override CARGO_PROFILE_DEV_DEBUG := 0
FFMPEG_SOURCE_ARCHIVE := $(CURDIR)/target/ffmpeg-source-cache/ffmpeg.tar.gz
ONNX_RUNTIME_ARCHIVE_DIR := $(CURDIR)/target/speakers-analyze-runtime-cache
SERVICE_LEGACY_EVIDENCE_ROOT ?= core/fixtures/service_legacy_evidence
IOS_TARGET := aarch64-apple-ios
WINDOWS_TARGET := x86_64-pc-windows-msvc
RUST_HOST_EXCLUDES := --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-vad-analyze
# These four library harnesses are intentionally full-gate suites: each was
# measured above ten seconds and already has an explicit registry entry. Keep
# routine Clippy broad, but leave their execution to ci-full.
RUST_ROUTINE_EXCLUDES := $(RUST_HOST_EXCLUDES) --exclude solstone-core-sol-link --exclude solstone-core-convey-body --exclude solstone-core-facets --exclude solstone-core-describe

# Every crate RUST_HOST_EXCLUDES removes from the workspace test selection is
# named here, and check-rust-onnx-test runs it. They are excluded because they
# link the pinned host ONNX Runtime, which a plain workspace `cargo test`
# cannot resolve -- but excluding them from the SELECTION also excluded them
# from the GATE. Measured 2026-08-11 on this tree: check-rust-test runs 4,411
# tests and NOT ONE of the 33 in solstone-core-speakers-analyze (26) and
# solstone-core-speakers-onnx (7) was among them, while
# check-rust-vad-analyze-test covered only the third crate. Tests nobody runs
# are worse than no tests, because they read as coverage.
# ci_gate_purity::every_host_excluded_crate_is_tested_by_a_ci_target keeps the
# two lists in step: adding a fourth --exclude without adding it here reds.
ONNX_HOST_TEST_PACKAGES := -p solstone-core-speakers-analyze -p solstone-core-speakers-onnx -p solstone-core-vad-analyze

# Only supervisor tests whose positive waits classify load-dilated exhaustion as
# explicit inconclusive outcomes belong here. The two supervisor-domain raw-poll
# tests (supervisor_boot and supervisor_providers), the two session races
# (cogitate_session and generate_session), and the two non-race tests
# (convey_restart_no_python_spawn and convey_process) remain out of scope because
# load could make their hard assertions report a false FAILED.
RUST_RACE_TEST_TARGETS := --test supervisor_app_stack --test supervisor_shutdown --test supervisor_tick
RUST_RACE_RUNS ?= 5
RUST_RACE_LOAD_JOBS ?= 12

# bindgen (inside ffmpeg-sys-next, which solstone-core-describe pulls in) asks
# libclang for its builtin-header directory, and libclang derives that path from
# its own .so location. On Fedora libclang ships in /usr/lib64 while the headers
# ship in /usr/lib/clang/<ver>/include, so the derived path misses and every
# glibc header doing `#include_next <limits.h>` dies with
#   fatal error: 'limits.h' file not found
# taking check-rust-msrv, -clippy, -test and build down with it.
#
# The wildcard is empty on hosts whose resource dir already resolves (macOS,
# Debian/Ubuntu), so this is a no-op there rather than a second opinion.
#
# ⚠ SCOPED TO THE HOST RUST TARGETS, and that scoping is load-bearing — it was
# a global `export` for one commit and it BROKE the wheel build, because the
# wheel cross-compiles with zig and a host clang include path injected into a
# cross-compile makes bindgen fail to find uint8_t. Measured both ways:
# A host Rust target that compiles the workspace fails with the global export and succeeds
# with CLANG_BUILTIN_INCLUDE= empty.
#
# ⛔ Do not widen this back to a global export. The wheel recipes must NOT
# inherit it.
#
# ⚠ EVERY host Rust target that compiles the workspace belongs on the list below,
# and a missing one fails as `ffmpeg-sys-next` exit 101 -- which reads exactly
# like a test failure. check-rust-onnx-test was added to ci without being added
# here and died that way on Fedora; the falsification that "proved" the target
# worked had exported this variable in its own shell, so it measured the shell
# rather than the recipe.
CLANG_BUILTIN_INCLUDE := $(firstword $(wildcard /usr/lib/clang/*/include /usr/lib64/clang/*/include))
ifneq ($(CLANG_BUILTIN_INCLUDE),)
# install builds the solstone-core wheel through maturin, and solstone-core now
# depends transitively on ffmpeg-sys-next (via solstone-core-grab), whose build
# script needs these args to find limits.h. Leaving install off this list made
# `make install` fail on a clean environment while every Rust gate stayed green,
# because the gates carry the export and install itself must carry it too.
install .installed build check-rust-msrv check-rust-clippy check-rust-clippy-full check-rust-unit check-rust-doc check-rust-test check-rust-describe-cli-stubs check-rust-race check-rust-onnx-test check-rust-registry-suite check-rust-registry-package check-rust-shipped-binaries check-rust-release-manifest audit ci-full-prep-cargo: export BINDGEN_EXTRA_CLANG_ARGS := -I$(CLANG_BUILTIN_INCLUDE)
endif
REQUIRE_CARGO := command -v cargo >/dev/null 2>&1 || { echo "cargo is required for Rust checks; install cargo and retry" >&2; exit 1; }
REQUIRE_RUSTUP := command -v rustup >/dev/null 2>&1 || { echo "rustup is required for platform gates; install rustup and retry" >&2; exit 1; }
# Prep measured, rather than merely anticipated, that a host GNU cargo build of
# solstone-core-speakers-analyze reaches GLIBC_2.34. These zig-GNU maturin args
# are therefore the checked-in developer path for the helper's GLIBC_2.27 floor.
SPEAKERS_ANALYZE_LINUX_X86_64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target x86_64-unknown-linux-gnu
SPEAKERS_ANALYZE_LINUX_AARCH64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target aarch64-unknown-linux-gnu
# The VAD helper bundles the SAME pinned CPU ONNX Runtime as the speakers
# helper (identical `ort` pin, identical onnxruntime 1.25.0 bytes), so it shares
# the GLIBC_2.27 floor and the same zig-GNU cross args.
VAD_ANALYZE_LINUX_X86_64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target x86_64-unknown-linux-gnu
VAD_ANALYZE_LINUX_AARCH64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target aarch64-unknown-linux-gnu
PDF_LINUX_X86_64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target x86_64-unknown-linux-gnu
PDF_LINUX_AARCH64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target aarch64-unknown-linux-gnu
# Host link inputs for the ONNX-bundling helpers. There is exactly ONE staged
# runtime directory per target — solstone-distribution acquire onnx owns the
# pinned URL/digest table — and the VAD targets reuse it rather than
# provisioning a second copy. Only the wheel *payload* is per-package, because
# each helper's build.rs rpath points at its own $ORIGIN/../lib/<package>.
# These inputs define what the native integrity gate measures. Protect the
# shell, physical repository root, host probes, and pinned mapping from Make's
# command-line variable precedence so a caller cannot redirect or narrow the
# gate while still receiving a successful result.
override SHELL := /bin/sh
override .SHELLFLAGS := -c
override REPO_ROOT := $(shell /bin/pwd -P)
override HOST_SYSTEM := $(shell /usr/bin/uname -s)
override HOST_ARCH := $(shell /usr/bin/uname -m)
override ONNX_RUNTIME_LINUX_X86_64_TARGET := linux-x86_64
override ONNX_RUNTIME_LINUX_X86_64_LINK_NAMES := libonnxruntime.so.1.25.0 libonnxruntime.so.1 libonnxruntime.so
override ONNX_RUNTIME_LINUX_X86_64_DIGEST := 6976c9c6b2db120e835a7091e2f4bd2308a76d3856a7181beb7e7a9b1e08f9e5
override ONNX_RUNTIME_LINUX_AARCH64_TARGET := linux-aarch64
override ONNX_RUNTIME_LINUX_AARCH64_LINK_NAMES := libonnxruntime.so.1.25.0 libonnxruntime.so.1 libonnxruntime.so
override ONNX_RUNTIME_LINUX_AARCH64_DIGEST := d47425026b2474e1deb0b8cf22f74cd943539af85873aa3fb8052862445beef3
override ONNX_RUNTIME_MACOS_ARM64_TARGET := macos-arm64
override ONNX_RUNTIME_MACOS_ARM64_LINK_NAMES := libonnxruntime.1.25.0.dylib libonnxruntime.dylib
override ONNX_RUNTIME_MACOS_ARM64_DIGEST := bafe7d3f3fa8e31195501e5694e73ef240708d5df039feb272b8d506d2783a74
override ONNX_RUNTIME_HOST_TARGET :=
override ONNX_RUNTIME_HOST_LINK_NAMES :=
override ONNX_RUNTIME_HOST_DIGEST :=
override ONNX_RUNTIME_HOST_HASH_PROGRAM :=
override ONNX_RUNTIME_HOST_HASH_ARGS :=
override ONNX_RUNTIME_HOST_LOADER_ENV :=

ifeq ($(HOST_SYSTEM),Linux)
ifneq ($(filter x86_64 amd64,$(HOST_ARCH)),)
override ONNX_RUNTIME_HOST_TARGET := $(ONNX_RUNTIME_LINUX_X86_64_TARGET)
override ONNX_RUNTIME_HOST_DIGEST := $(ONNX_RUNTIME_LINUX_X86_64_DIGEST)
override ONNX_RUNTIME_HOST_LINK_NAMES := $(ONNX_RUNTIME_LINUX_X86_64_LINK_NAMES)
else ifneq ($(filter aarch64 arm64,$(HOST_ARCH)),)
override ONNX_RUNTIME_HOST_TARGET := $(ONNX_RUNTIME_LINUX_AARCH64_TARGET)
override ONNX_RUNTIME_HOST_DIGEST := $(ONNX_RUNTIME_LINUX_AARCH64_DIGEST)
override ONNX_RUNTIME_HOST_LINK_NAMES := $(ONNX_RUNTIME_LINUX_AARCH64_LINK_NAMES)
endif
override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/sha256sum
override ONNX_RUNTIME_HOST_LOADER_ENV := LD_LIBRARY_PATH
else ifeq ($(HOST_SYSTEM),Darwin)
ifneq ($(filter arm64 aarch64,$(HOST_ARCH)),)
override ONNX_RUNTIME_HOST_TARGET := $(ONNX_RUNTIME_MACOS_ARM64_TARGET)
override ONNX_RUNTIME_HOST_DIGEST := $(ONNX_RUNTIME_MACOS_ARM64_DIGEST)
override ONNX_RUNTIME_HOST_LINK_NAMES := $(ONNX_RUNTIME_MACOS_ARM64_LINK_NAMES)
override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/shasum
override ONNX_RUNTIME_HOST_HASH_ARGS := -a 256
override ONNX_RUNTIME_HOST_LOADER_ENV := DYLD_LIBRARY_PATH
endif
endif

override ONNX_RUNTIME_HOST_LINK_DIR := $(REPO_ROOT)/target/speakers-analyze-runtime-link/$(ONNX_RUNTIME_HOST_TARGET)
override ONNX_RUNTIME_HASH_PROBE_TEXT := solstone checksum verifier probe
override ONNX_RUNTIME_HASH_PROBE_DIGEST := 1629c6bcea388b9f721343d214f545f712c0bc70ed9f34866a08b0f8ccb2edb7

ifeq ($(ONNX_RUNTIME_HOST_LOADER_ENV),DYLD_LIBRARY_PATH)
# macOS system binaries strip DYLD_* from their inherited environment under
# SIP. Preserve an explicit Make value (`make DYLD_LIBRARY_PATH=...`) instead
# of pretending a value stripped before the recipe shell is still observable.
override VAD_ANALYZE_HOST_ORT_ENV := ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)" DYLD_LIBRARY_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)$(if $(DYLD_LIBRARY_PATH),:$(DYLD_LIBRARY_PATH))"
else
override VAD_ANALYZE_HOST_ORT_ENV := ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)" LD_LIBRARY_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)$${LD_LIBRARY_PATH:+:$$LD_LIBRARY_PATH}"
endif

REQUIRE_SUPPORTED_ONNX_HOST = test -n "$(ONNX_RUNTIME_HOST_TARGET)" || { echo "unsupported host for the pinned ONNX Runtime: observed $(HOST_SYSTEM)/$(HOST_ARCH); supported: Linux/x86_64, Linux/aarch64, Darwin/arm64" >&2; exit 1; }

# Define a shell function that first proves the selected digest instrument on
# fixed known input, then judges every link name from the pinned target table.
# Return 20 when the instrument is broken and 10 when staged data is invalid;
# callers can therefore prescribe a repair that can actually fix the failure.
define DEFINE_ONNX_RUNTIME_VALIDATOR
validate_onnx_runtime() { \
	runtime_dir="$$1"; \
	repair_command="$$2"; \
	require_regular_file="$$3"; \
	validation_error=''; \
	if [ ! -x "$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" ]; then \
		validation_error="ONNX Runtime checksum verifier is unavailable: $(ONNX_RUNTIME_HOST_HASH_PROGRAM); install or repair that verifier and retry"; \
		return 20; \
	fi; \
	probe_file=$$(mktemp "$${TMPDIR:-/var/tmp}/solstone-onnx-hash-probe-XXXXXX") || { validation_error='could not create checksum verifier probe file'; return 20; }; \
	printf '%s\n' '$(ONNX_RUNTIME_HASH_PROBE_TEXT)' > "$$probe_file"; \
	if probe_output=$$("$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" $(ONNX_RUNTIME_HOST_HASH_ARGS) "$$probe_file" 2>&1); then probe_status=0; else probe_status=$$?; fi; \
	rm -f "$$probe_file"; \
	probe_digest=$${probe_output%%[[:space:]]*}; \
	if [ "$$probe_status" -ne 0 ] || [ "$$probe_digest" != "$(ONNX_RUNTIME_HASH_PROBE_DIGEST)" ]; then \
		validation_error="ONNX Runtime checksum verifier failed its known-input check: $(ONNX_RUNTIME_HOST_HASH_PROGRAM) $(ONNX_RUNTIME_HOST_HASH_ARGS); install or repair that verifier and retry"; \
		return 20; \
	fi; \
	for library in $(ONNX_RUNTIME_HOST_LINK_NAMES); do \
		library_path="$$runtime_dir/$$library"; \
		if [ ! -f "$$library_path" ] || [ ! -r "$$library_path" ]; then \
			validation_error="invalid pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual missing, non-file, or unreadable); run '$$repair_command' and retry"; \
			return 10; \
		fi; \
		if [ "$$require_regular_file" = true ] && [ -L "$$library_path" ]; then \
			validation_error="invalid pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual symbolic link; regular file required); run '$$repair_command' and retry"; \
			return 10; \
		fi; \
		if digest_output=$$("$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" $(ONNX_RUNTIME_HOST_HASH_ARGS) "$$library_path" 2>&1); then digest_status=0; else digest_status=$$?; fi; \
		actual_digest=$${digest_output%%[[:space:]]*}; \
		if [ "$$digest_status" -ne 0 ] || [ -z "$$actual_digest" ]; then \
			validation_error="could not checksum pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual checksum input failure); run '$$repair_command' and retry"; \
			return 10; \
		fi; \
		if [ "$$actual_digest" != "$(ONNX_RUNTIME_HOST_DIGEST)" ]; then \
			validation_error="invalid pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual $$actual_digest); run '$$repair_command' and retry"; \
			return 10; \
		fi; \
	done; \
	return 0; \
}
endef

REQUIRE_ONNX_HOST_RUNTIME = $(DEFINE_ONNX_RUNTIME_VALIDATOR); if validate_onnx_runtime "$(ONNX_RUNTIME_HOST_LINK_DIR)" "make check-rust-onnx-stage" false; then :; else validation_status=$$?; echo "$$validation_error" >&2; exit "$$validation_status"; fi
REQUIRE_SANDBOX_PROCESSING_PAYLOAD = $(DEFINE_ONNX_RUNTIME_VALIDATOR); if validate_onnx_runtime "$(RUST_TARGET_DIR)/lib/solstone-core-speakers-analyze" "make build-sandbox-processing" true; then :; else validation_status=$$?; echo "$$validation_error" >&2; exit "$$validation_status"; fi
override PDF_RUNTIME_HOST_TARGET :=
override PDF_RUNTIME_HOST_LIBRARY :=
override PDF_RUNTIME_HOST_DIGEST :=
override PDF_RUNTIME_LINUX_X86_64_DIGEST := 687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e
override PDF_RUNTIME_LINUX_AARCH64_DIGEST := 933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3
override PDF_RUNTIME_MACOS_ARM64_DIGEST := df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9
ifeq ($(HOST_SYSTEM),Linux)
ifneq ($(filter x86_64 amd64,$(HOST_ARCH)),)
override PDF_RUNTIME_HOST_TARGET := linux-x86_64
override PDF_RUNTIME_HOST_LIBRARY := libpdfium.so
override PDF_RUNTIME_HOST_DIGEST := $(PDF_RUNTIME_LINUX_X86_64_DIGEST)
else ifneq ($(filter aarch64 arm64,$(HOST_ARCH)),)
override PDF_RUNTIME_HOST_TARGET := linux-aarch64
override PDF_RUNTIME_HOST_LIBRARY := libpdfium.so
override PDF_RUNTIME_HOST_DIGEST := $(PDF_RUNTIME_LINUX_AARCH64_DIGEST)
endif
else ifeq ($(HOST_SYSTEM),Darwin)
ifneq ($(filter arm64 aarch64,$(HOST_ARCH)),)
override PDF_RUNTIME_HOST_TARGET := macos-arm64
override PDF_RUNTIME_HOST_LIBRARY := libpdfium.dylib
override PDF_RUNTIME_HOST_DIGEST := $(PDF_RUNTIME_MACOS_ARM64_DIGEST)
endif
endif
override PDF_RUNTIME_HOST_LINK_DIR := $(REPO_ROOT)/target/pdfium-runtime-link/$(PDF_RUNTIME_HOST_TARGET)
REQUIRE_SUPPORTED_PDF_HOST = test -n "$(PDF_RUNTIME_HOST_TARGET)" || { echo "unsupported host for the pinned PDFium runtime: observed $(HOST_SYSTEM)/$(HOST_ARCH); supported: Linux/x86_64, Linux/aarch64, Darwin/arm64" >&2; exit 1; }

define DEFINE_PDF_RUNTIME_VALIDATOR
validate_pdf_runtime() { \
	validation_error=''; \
	if [ ! -x "$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" ]; then \
		validation_error="PDFium checksum verifier is unavailable: $(ONNX_RUNTIME_HOST_HASH_PROGRAM); install or repair that verifier and retry"; \
		return 20; \
	fi; \
	probe_file=$$(mktemp "$${TMPDIR:-/var/tmp}/solstone-pdfium-hash-probe-XXXXXX") || { validation_error='could not create checksum verifier probe file'; return 20; }; \
	printf '%s\n' '$(ONNX_RUNTIME_HASH_PROBE_TEXT)' > "$$probe_file"; \
	if probe_output=$$("$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" $(ONNX_RUNTIME_HOST_HASH_ARGS) "$$probe_file" 2>&1); then probe_status=0; else probe_status=$$?; fi; \
	rm -f "$$probe_file"; \
	probe_digest=$${probe_output%%[[:space:]]*}; \
	if [ "$$probe_status" -ne 0 ] || [ "$$probe_digest" != "$(ONNX_RUNTIME_HASH_PROBE_DIGEST)" ]; then \
		validation_error="PDFium checksum verifier failed its known-input check: $(ONNX_RUNTIME_HOST_HASH_PROGRAM) $(ONNX_RUNTIME_HOST_HASH_ARGS); install or repair that verifier and retry"; \
		return 20; \
	fi; \
	library_path="$(PDF_RUNTIME_HOST_LINK_DIR)/$(PDF_RUNTIME_HOST_LIBRARY)"; \
	if [ ! -f "$$library_path" ] || [ ! -r "$$library_path" ]; then \
		validation_error="invalid pinned host PDFium runtime file: $$library_path (expected sha256 $(PDF_RUNTIME_HOST_DIGEST), actual missing, non-file, or unreadable); run 'make check-rust-pdf-stage' and retry"; \
		return 10; \
	fi; \
	if digest_output=$$("$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" $(ONNX_RUNTIME_HOST_HASH_ARGS) "$$library_path" 2>&1); then digest_status=0; else digest_status=$$?; fi; \
	actual_digest=$${digest_output%%[[:space:]]*}; \
	if [ "$$digest_status" -ne 0 ] || [ -z "$$actual_digest" ]; then \
		validation_error="could not checksum pinned host PDFium runtime file: $$library_path (expected sha256 $(PDF_RUNTIME_HOST_DIGEST), actual checksum input failure); run 'make check-rust-pdf-stage' and retry"; \
		return 10; \
	fi; \
	if [ "$$actual_digest" != "$(PDF_RUNTIME_HOST_DIGEST)" ]; then \
		validation_error="invalid pinned host PDFium runtime file: $$library_path (expected sha256 $(PDF_RUNTIME_HOST_DIGEST), actual $$actual_digest); run 'make check-rust-pdf-stage' and retry"; \
		return 10; \
	fi; \
	return 0; \
}
endef

REQUIRE_PDF_HOST_RUNTIME = $(REQUIRE_SUPPORTED_PDF_HOST); $(DEFINE_PDF_RUNTIME_VALIDATOR); if validate_pdf_runtime; then :; else validation_status=$$?; echo "$$validation_error" >&2; exit "$$validation_status"; fi

# Require uv only for goals that actually use it. `preflight` is a pure
# stdlib readiness battery and `install` runs preflight as its own fail-fast
# pre-step, so neither should abort at parse time when uv is absent — they
# report uv-absence themselves. Rust-only and retired/gated goals are likewise
# optional; Python-dependent goals outside this list still abort at parse time.
UV := $(shell command -v uv 2>/dev/null)
UV_OPTIONAL_GOALS := \
	preflight install \
	check-rust-fmt check-rust-msrv check-rust-clippy check-rust-clippy-full \
	check-rust-unit check-rust-doc check-rust-test check-rust-race \
	check-rust-ios check-rust-macos check-rust-windows check-rust-deny check-rust-describe-cli-stubs \
	require-win-remote-host sync-win-host win-host-ci \
	check-rust-vad-analyze-test build-sandbox-processing check-rust-sandbox-processing-build check-rust-onnx-stage check-rust-onnx-ready check-rust-onnx-test \
	check-rust-pdf-stage check-rust-pdf-ready check-rust-pdf-test $(PDF_RUNTIME_HOST_LINK_DIR) \
	check-rust-registry-suite check-rust-registry-package check-rust-shipped-binaries \
	check-rust-ci-topology ci ci-under-poison ci-full ci-full-under-poison ci-full-plan \
	ci-contained ci-prep-ffmpeg \
	ci-full-prep ci-full-prep-cargo ci-full-prep-onnx ci-full-prep-pdf \
	verify test build format format-check report-rust-code-evidence \
	check-service-legacy-evidence service-legacy-evidence-capture audit \
	test-cov test-integration test-performance test-app test-only watch coverage
ifndef UV
ifneq ($(filter-out $(UV_OPTIONAL_GOALS),$(MAKECMDGOALS)),)
$(error uv is not installed. Install it: curl -LsSf https://astral.sh/uv/install.sh | sh)
endif
endif

# --- Retired Python test rails ----------------------------------------------
define RETIRED_PYTHON_TEST_RAIL
	@echo "$(1): retired. Use the Rust test hierarchy documented in AGENTS.md." >&2
	@exit 1
endef

# TRANSPARENCY_GUARD: the transparency rail is gated behind
# TRANSPARENCY_ACTIVATED, checked in inactive (0). Set TRANSPARENCY_ACTIVATED=1
# to restore the real implementation. See docs/PORTING.md.
TRANSPARENCY_ACTIVATED ?= 0
export TRANSPARENCY_ACTIVATED

define TRANSPARENCY_GUARD
	@echo "$(1): transparency rail inactive; set TRANSPARENCY_ACTIVATED=1 to restore (see docs/PORTING.md)" >&2
	@exit 1
endef

# User bin directory for symlink (standard location, usually already in PATH)
USER_BIN := $(HOME)/.local/bin

.python-version-hash: FORCE
	@tmp_file=$$(mktemp); \
	python3 -c "import sys; print(sys.version_info[:2])" > "$$tmp_file"; \
	if [ ! -f $@ ] || ! cmp -s "$$tmp_file" $@; then mv "$$tmp_file" $@; else rm -f "$$tmp_file"; fi

# The native `solstone-core` binary is built by maturin from Rust sources that
# live OUTSIDE its package directory — packages/solstone-core/pyproject.toml
# points maturin at ../../core. So none of `.installed`'s other prerequisites
# move when native code changes: `.installed` stays satisfied, `uv sync` never
# runs, and .venv/bin/solstone-core keeps serving whatever was built at first
# install. Every Python test and check that shells out to that binary then
# exercises stale native bytes while reporting green. This stamp puts the Rust
# tree into the prerequisite chain; the `cache-keys` block in
# packages/solstone-core/pyproject.toml is what makes the `uv sync` this
# triggers actually rebuild. Hashed by content, not mtime, so a checkout or a
# touch that changes nothing does not force a reinstall. core/target/ is
# deliberately absent — the build writes it, so keying on it never settles.
.rust-core-hash: FORCE
	@tmp_file=$$(mktemp); \
	python3 -c 'import hashlib, pathlib; root = pathlib.Path("core"); pats = ("Cargo.toml", "Cargo.lock", "crates/**/Cargo.toml", "crates/**/*.rs", "fixtures/**/*"); paths = sorted({p for pat in pats for p in root.glob(pat) if p.is_file()}); print(hashlib.sha256(b"".join(str(p).encode() + b"\0" + p.read_bytes() + b"\0" for p in paths)).hexdigest())' > "$$tmp_file"; \
	if [ ! -f $@ ] || ! cmp -s "$$tmp_file" $@; then mv "$$tmp_file" $@; else rm -f "$$tmp_file"; fi

# Marker file to track installation
.installed: pyproject.toml uv.lock .python-version-hash .rust-core-hash
	@echo "Installing the Python development environment with uv..."
	$(UV) sync --group dev
	@touch .installed

# Generate lock file if missing
uv.lock: pyproject.toml
	$(UV) lock

# Hopper lode setup. Hopper prefers this target over `install` and its contract
# is deliberately lean: the dependencies needed to EDIT the tree and run the
# unit gate, and nothing else. Host runtime provisioning and large artifact
# downloads stay in `install`.
#
# `make ci` is fmt + CI topology + Clippy + unit tests, all cargo, and none of
# those targets depends on `.installed`. The three ONNX crates are outside the
# unit gate via RUST_HOST_EXCLUDES, so the native runtime stages are `install`'s
# business rather than a lode's. A populated cargo registry is therefore the
# whole requirement -- and it must be populated, because the CI topology check
# runs `cargo run --locked --offline`.
.PHONY: hopper-install
hopper-install: ci-full-prep-cargo

# Retired. The journal installs from the distribution tree. Developers use
# cargo. Hopper lodes use hopper-install. Remaining Python repo-maintenance
# scripts still depend on `.installed` and `uv sync`; do not treat this
# target's refusal as a regression those scripts caused.
# A zero-byte gitignored `.installed` predating the cut makes `.installed` a
# no-op on any machine that already ran it — `rm -f .installed` first or the
# tree lies.
install:
	@echo "Error: 'make install' is retired. Install the journal from the distribution tree; develop with cargo; hopper lodes use hopper-install." >&2
	@exit 1

# Staging the shared host runtime is BUILD-TIME tooling: it shells to Python, so
# it stays OUTSIDE both poisoned Rust gates, which cannot shell to an interpreter at
# all. Validate every required link and its pinned digest on every invocation;
# only invalid data invokes the existing staging operation. A healthy checkout
# is therefore a no-op, while a surviving directory can no longer hide a
# missing or corrupt library. Staging remains a single-writer operation, as it
# was before this target became a validator; validation and Cargo consumption
# are not an atomic snapshot.
check-rust-onnx-stage:
	@$(REQUIRE_CARGO)
	@set -u; \
	$(REQUIRE_SUPPORTED_ONNX_HOST); \
	$(DEFINE_ONNX_RUNTIME_VALIDATOR); \
	if validate_onnx_runtime "$(ONNX_RUNTIME_HOST_LINK_DIR)" "make check-rust-onnx-stage" false; then \
		echo "host ONNX Runtime verified at $(ONNX_RUNTIME_HOST_LINK_DIR)"; \
		exit 0; \
	else \
		validation_status=$$?; \
	fi; \
	if [ "$$validation_status" -eq 20 ]; then \
		echo "$$validation_error" >&2; \
		exit "$$validation_status"; \
	fi; \
	echo "$$validation_error" >&2; \
	echo "repairing the pinned host ONNX Runtime stage" >&2; \
	if ! $(SOLSTONE_DISTRIBUTION_ACQUIRE) acquire onnx --target $(ONNX_RUNTIME_HOST_TARGET) --package-dir target/runtime-package-staging/solstone-core-vad-analyze --receipt target/vad-analyze-runtime-provenance/$(ONNX_RUNTIME_HOST_TARGET).json; then \
		echo "failed to stage the pinned host ONNX Runtime" >&2; \
		exit 1; \
	fi; \
	if validate_onnx_runtime "$(ONNX_RUNTIME_HOST_LINK_DIR)" "make check-rust-onnx-stage" false; then \
		echo "host ONNX Runtime staged and verified at $(ONNX_RUNTIME_HOST_LINK_DIR)"; \
	else \
		validation_status=$$?; \
		echo "$$validation_error" >&2; \
		exit "$$validation_status"; \
	fi

# Staging PDFium fetches a digest-pinned archive and verifies a GitHub
# attestation, so it stays OUTSIDE both poisoned Rust gates. The runtime-loaded
# crate itself remains in ordinary host Cargo selection; only its real binary
# tests require this stage.
$(PDF_RUNTIME_HOST_LINK_DIR):
	$(SOLSTONE_DISTRIBUTION_ACQUIRE) acquire pdfium --target $(PDF_RUNTIME_HOST_TARGET) --package-dir target/runtime-package-staging/solstone-core-pdf --receipt target/pdfium-runtime-provenance/$(PDF_RUNTIME_HOST_TARGET).json

check-rust-pdf-stage:
	@$(REQUIRE_CARGO)
	@$(REQUIRE_SUPPORTED_PDF_HOST)
	$(SOLSTONE_DISTRIBUTION_ACQUIRE) acquire pdfium --target $(PDF_RUNTIME_HOST_TARGET) --package-dir target/runtime-package-staging/solstone-core-pdf --receipt target/pdfium-runtime-provenance/$(PDF_RUNTIME_HOST_TARGET).json
	@set -eu; $(REQUIRE_PDF_HOST_RUNTIME)
	@echo "host PDFium runtime staged and verified at $(PDF_RUNTIME_HOST_LINK_DIR)"

.PHONY: ci-prep-ffmpeg ci-full-prep ci-full-prep-cargo ci-full-prep-onnx ci-full-prep-pdf
.NOTPARALLEL: ci-full-prep
ci ci-contained ci-under-poison ci-prep-ffmpeg: export SOLSTONE_FFMPEG_SOURCE_ARCHIVE := $(FFMPEG_SOURCE_ARCHIVE)
ci ci-contained ci-under-poison ci-prep-ffmpeg: export SOLSTONE_DISTRIBUTION_OFFLINE := 1
ci-full ci-full-under-poison ci-full-prep ci-full-prep-cargo: export SOLSTONE_FFMPEG_SOURCE_ARCHIVE := $(FFMPEG_SOURCE_ARCHIVE)
ci-full ci-full-under-poison ci-full-prep ci-full-prep-cargo: export SOLSTONE_DISTRIBUTION_OFFLINE := 1
ci-full ci-full-under-poison ci-full-prep check-rust-distribution check-rust-distribution-under-poison: export SOLSTONE_DISTRIBUTION_ONNX_ARCHIVE_DIR := $(ONNX_RUNTIME_ARCHIVE_DIR)

# Preparation is the only CI surface allowed to fetch Cargo inputs or repair
# native runtime stages. The contained validation runners themselves stay
# offline.
ci-prep-ffmpeg:
	@$(REQUIRE_CARGO)
	$(SOLSTONE_DISTRIBUTION_ACQUIRE) acquire ffmpeg --dest $(FFMPEG_SOURCE_ARCHIVE)

ci-full-prep: ci-full-prep-cargo ci-full-prep-onnx ci-full-prep-pdf

ci-full-prep-cargo: ci-prep-ffmpeg
	@$(REQUIRE_CARGO)
	cargo fetch --manifest-path $(RUST_MANIFEST) --locked
	cargo check --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --lib --bins --locked
	cargo test --manifest-path $(RUST_MANIFEST) --workspace $(RUST_ROUTINE_EXCLUDES) --lib --bins --no-run --locked

ci-full-prep-onnx:
	@$(MAKE) --no-print-directory CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 check-rust-onnx-stage

ci-full-prep-pdf:
	@$(MAKE) --no-print-directory CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 check-rust-pdf-stage

# Read-only native-runtime readiness checks. The runner may verify prepared
# inputs, but only the explicit prep targets above are allowed to repair them.
check-rust-onnx-ready:
	@set -eu; $(REQUIRE_ONNX_HOST_RUNTIME)
	@echo "host ONNX Runtime ready at $(ONNX_RUNTIME_HOST_LINK_DIR)"

# This is the tree's first proof that the helpers' compiled rpath actually
# resolves. Every other ONNX gate runs with $(VAD_ANALYZE_HOST_ORT_ENV), which
# supplies a loader override; this check deliberately does not rebuild because
# rebuilding would repair its own negative case.
build-sandbox-processing:
	@$(REQUIRE_CARGO)
	@set -eu; \
	target_dir="$(RUST_TARGET_DIR)"; \
	[ -n "$$target_dir" ] || { echo "build-sandbox-processing requires a non-empty RUST_TARGET_DIR" >&2; exit 2; }; \
	[ "$$target_dir" != / ] || { echo "build-sandbox-processing refuses RUST_TARGET_DIR=/" >&2; exit 2; }; \
	payload_dir="$$target_dir/lib/solstone-core-speakers-analyze"; \
	if [ -L "$$target_dir/lib" ]; then echo "build-sandbox-processing refuses symlinked target library directory: $$target_dir/lib" >&2; exit 1; fi; \
	if [ -L "$$payload_dir" ]; then echo "build-sandbox-processing refuses symlinked payload directory: $$payload_dir" >&2; exit 1; fi; \
	$(MAKE) --no-print-directory check-rust-onnx-stage; \
	$(VAD_ANALYZE_HOST_ORT_ENV) cargo build --manifest-path $(RUST_MANIFEST) -p solstone-core-speakers-analyze -p solstone-core-vad-analyze --locked; \
	rm -rf -- "$$payload_dir"; \
	mkdir -p "$$payload_dir"; \
	for library in $(ONNX_RUNTIME_HOST_LINK_NAMES); do \
		cp "$(ONNX_RUNTIME_HOST_LINK_DIR)/$$library" "$$payload_dir/$$library"; \
	done; \
	$(REQUIRE_SANDBOX_PROCESSING_PAYLOAD)

check-rust-sandbox-processing-build:
	@set -eu; \
	$(REQUIRE_SUPPORTED_ONNX_HOST); \
	$(REQUIRE_SANDBOX_PROCESSING_PAYLOAD); \
	for helper in solstone-core-speakers-analyze solstone-core-vad-analyze; do \
		helper_path="$(RUST_TARGET_DIR)/debug/$$helper"; \
		[ -x "$$helper_path" ] || { echo "sandbox processing helper is missing or not executable: $$helper_path; run 'make build-sandbox-processing' and retry" >&2; exit 1; }; \
	done; \
	if output=$$(env -u LD_LIBRARY_PATH -u DYLD_LIBRARY_PATH "$(RUST_TARGET_DIR)/debug/solstone-core-speakers-analyze" </dev/null 2>&1); then \
		echo "solstone-core-speakers-analyze unexpectedly accepted an empty request" >&2; \
		exit 1; \
	else \
		run_status=$$?; \
	fi; \
	[ "$$run_status" -eq 64 ] || { echo "solstone-core-speakers-analyze exited $$run_status, expected usage exit 64" >&2; echo "$$output" >&2; exit 1; }; \
	case "$$output" in \
		*'"schema":"solstone-speaker-analyze-error-v1"'*'"reason":"malformed-request"'*) ;; \
		*) echo "solstone-core-speakers-analyze did not emit its malformed-request record" >&2; echo "$$output" >&2; exit 1 ;; \
	esac; \
	if output=$$(env -u LD_LIBRARY_PATH -u DYLD_LIBRARY_PATH "$(RUST_TARGET_DIR)/debug/solstone-core-vad-analyze" </dev/null 2>&1); then \
		echo "solstone-core-vad-analyze unexpectedly accepted an empty request" >&2; \
		exit 1; \
	else \
		run_status=$$?; \
	fi; \
	[ "$$run_status" -eq 64 ] || { echo "solstone-core-vad-analyze exited $$run_status, expected usage exit 64" >&2; echo "$$output" >&2; exit 1; }; \
	case "$$output" in \
		*'"schema":"solstone-vad-error-v1"'*'"reason":"malformed-request"'*) ;; \
		*) echo "solstone-core-vad-analyze did not emit its malformed-request record" >&2; echo "$$output" >&2; exit 1 ;; \
	esac

check-rust-pdf-ready:
	@set -eu; $(REQUIRE_PDF_HOST_RUNTIME)
	@echo "host PDFium runtime ready at $(PDF_RUNTIME_HOST_LINK_DIR)"

# The ONNX-linked crates' own #[test]s. This runs INSIDE ci: it requires the
# staged runtime rather than building it, which is exactly the contract
# check-rust-shipped-binaries has carried since it started building the shipped
# helpers. Serialized to match check-rust-test -- three ONNX-linked suites
# loading models in parallel would measure the host, not the product.
check-rust-onnx-test:
	@$(REQUIRE_CARGO)
	@set -eu; \
	if [ "$$(uname -s)" != "Linux" ]; then \
		echo "ONNX-linked crate tests: not run on $$(uname -s); these helpers ship on Linux"; \
		exit 0; \
	fi; \
	$(REQUIRE_ONNX_HOST_RUNTIME); \
	$(VAD_ANALYZE_HOST_ORT_ENV) cargo test --manifest-path $(RUST_MANIFEST) $(ONNX_HOST_TEST_PACKAGES) --locked -- --test-threads=1

check-rust-pdf-test:
	@$(REQUIRE_CARGO)
	@set -eu; \
	if [ "$$(uname -s)" != "Linux" ]; then \
		echo "PDFium crate tests: not run on $$(uname -s); this helper ships only on Linux"; \
		exit 0; \
	fi; \
	$(REQUIRE_PDF_HOST_RUNTIME); \
	SOLSTONE_CORE_PDF_LIBRARY="$(PDF_RUNTIME_HOST_LINK_DIR)/$(PDF_RUNTIME_HOST_LIBRARY)" cargo test --manifest-path $(RUST_MANIFEST) -p solstone-core-pdf --locked -- --test-threads=1

# Retained name: check-rust-shipped-binaries' recovery message named it for
# months and it is in muscle memory. It now stages AND runs all three crates.
check-rust-vad-analyze-test: check-rust-onnx-stage check-rust-onnx-test

check-rust-fmt:
	@$(REQUIRE_CARGO)
	cargo fmt --manifest-path $(RUST_MANIFEST) --all -- --check

check-rust-msrv:
	@$(REQUIRE_CARGO)
	@$(REQUIRE_RUSTUP)
	@rustup toolchain list 2>/dev/null | grep -Eq '^1\.95\.0(-|[[:space:]])' || { echo "Rust toolchain 1.95.0 is required for the MSRV gate; run rustup toolchain install 1.95.0" >&2; exit 1; }
	@set -eu; \
		mkdir -p "$(RUST_TARGET_DIR)"; \
		msrv_parent=$$(cd "$(RUST_TARGET_DIR)" && pwd -P); \
		msrv_target=$$(mktemp -d "$$msrv_parent/ci-msrv-1.95.0-XXXXXX"); \
		set +e; \
		CARGO_TARGET_DIR="$$msrv_target" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 RUSTUP_TOOLCHAIN=1.95.0 \
			cargo check --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --locked; \
		cargo_status=$$?; \
		set -e; \
		if $(CURDIR)/scripts/check_rust_target_live_use.sh "$$msrv_target" --cargo-environment; then \
			rm -rf -- "$$msrv_target"; \
		else \
			echo "MSRV target retained because a live process still uses it: $$msrv_target" >&2; \
		fi; \
		exit "$$cargo_status"

check-rust-clippy:
	@$(REQUIRE_CARGO)
	cargo clippy --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --lib --bins --locked --offline -- -D warnings

check-rust-clippy-full:
	@$(REQUIRE_CARGO)
	cargo clippy --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --all-targets --locked -- -D warnings

# Routine validation runs only in-process unit harnesses from workspace library
# and binary targets. This --lib --bins Clippy invocation does not select Cargo
# integration-test targets. `make ci` uses this Clippy command, while both
# public code commands use the unit target below; full validation selects its
# own registered evidence.
check-rust-unit:
	@$(REQUIRE_CARGO)
	cargo test --manifest-path $(RUST_MANIFEST) --workspace $(RUST_ROUTINE_EXCLUDES) --lib --bins --locked --offline --no-fail-fast -- --test-threads=1

.PHONY: report-rust-code-evidence
RUST_CODE_EVIDENCE_CONTEXT ?=
ifeq ($(RUST_CODE_EVIDENCE_CONTEXT),ci)
RUST_CODE_EVIDENCE_VALID := 1
RUST_CODE_EVIDENCE_CLIPPY := Clippy ran but unit tests did not: $(filter-out --exclude $(RUST_HOST_EXCLUDES),$(RUST_ROUTINE_EXCLUDES))
else ifeq ($(RUST_CODE_EVIDENCE_CONTEXT),test)
RUST_CODE_EVIDENCE_VALID := 1
RUST_CODE_EVIDENCE_CLIPPY := Routine Clippy did not run under this command.
else
RUST_CODE_EVIDENCE_VALID := 0
RUST_CODE_EVIDENCE_CLIPPY :=
endif
report-rust-code-evidence:
	@test "$(RUST_CODE_EVIDENCE_VALID)" = 1 || { echo "report-rust-code-evidence is internal; run 'make test' or 'make ci'" >&2; exit 2; }
	@echo "Code evidence ran the selected library/binary unit harnesses."
	@echo "Not unit-executed (RUST_ROUTINE_EXCLUDES): $(filter-out --exclude,$(RUST_ROUTINE_EXCLUDES))"
	@echo "$(RUST_CODE_EVIDENCE_CLIPPY)"
	@echo "Neither routine Clippy nor unit tests ran because native ONNX Runtime linkage is full-gate-only: $(filter-out --exclude,$(RUST_HOST_EXCLUDES))"
	@echo "Not run: Cargo integration targets, doctests, native/runtime/platform, dependency-policy, package/release, and other ci-full registry evidence."

check-rust-doc:
	@$(REQUIRE_CARGO)
	cargo test --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --doc --locked -- --test-threads=1

SOLSTONE_CI_RUNNER := cargo run --manifest-path $(RUST_MANIFEST) -p solstone-core-repository-contracts --bin solstone-ci --locked --offline --
SOLSTONE_DISTRIBUTION := cargo run --manifest-path $(RUST_MANIFEST) -p solstone-core-distribution --bin solstone-distribution --locked --offline --
# Acquire is the only distribution surface allowed to fetch. The producer
# itself stays offline and consumes the files this writes.
SOLSTONE_DISTRIBUTION_ACQUIRE := cargo run --manifest-path $(RUST_MANIFEST) -p solstone-core-distribution --bin solstone-distribution --locked --

# Export selector values directly instead of interpolating them into a shell
# command. This preserves comma- or space-separated values literally and keeps
# metacharacters from becoming shell syntax.
export SOLSTONE_CI_SETS := $(SETS)
export SOLSTONE_CI_AREAS := $(AREAS)
export SOLSTONE_CI_PACKAGES := $(PACKAGES)
export SOLSTONE_CI_TARGETS := $(TARGETS)
export SOLSTONE_CI_RECEIPT := $(RECEIPT)
export AUDIT_ADVISORY_BUNDLE := $(AUDIT_ADVISORY_BUNDLE)
export AUDIT_ADVISORY_RECEIPT := $(AUDIT_ADVISORY_RECEIPT)
export AUDIT_ADVISORY_PUBKEY := $(AUDIT_ADVISORY_PUBKEY)
export AUDIT_ADVISORY_LOCATOR := $(AUDIT_ADVISORY_LOCATOR)

.PHONY: check-rust-ci-topology ci-full-plan check-rust-clippy-full check-rust-doc check-rust-onnx-ready check-rust-pdf-ready check-rust-registry-suite check-rust-registry-package

check-rust-ci-topology:
	@$(REQUIRE_CARGO)
	$(SOLSTONE_CI_RUNNER) validate

.PHONY: check-rust-distribution check-rust-distribution-under-poison check-rust-distribution-cleanroom check-systemd-test
check-rust-distribution:
	$(call run-rust-gate-under-poison,check-rust-distribution-under-poison,$(DISTRIBUTION_FORBIDDEN_TOOLS))

# Live Docker/Podman oracle over already-produced x86_64 artifacts. Set
# SOLSTONE_DISTRIBUTION_OUT, the captured SOLSTONE_CLEANROOM_BUILDER_ID, a
# SOLSTONE_DISTRIBUTION_BIN built in that baseline builder, and, for remote
# Docker, DOCKER_HOST explicitly.
check-rust-distribution-cleanroom:
	core/distribution/cleanroom.sh --self-test
	core/distribution/cleanroom.sh "$${SOLSTONE_DISTRIBUTION_OUT:-/var/tmp/solstone-distribution-out}"

# Live systemd --user install of produced linux-x86_64 packages. Requires
# SOLSTONE_DIST_DIR and a working Docker/Podman daemon. This is the gate that
# observes Type=notify READY=1 and the real public-v1.0.22 crossover on both
# Debian/.deb and Fedora/.rpm. Explicit, not default make ci-full: it needs
# artifacts and privileged disposable containers.
check-systemd-test:
	@test -n "$$SOLSTONE_DIST_DIR" || { echo "check-systemd-test requires SOLSTONE_DIST_DIR (produced linux-x86_64 artifacts)" >&2; exit 2; }
	SOLSTONE_DIST_DIR="$$SOLSTONE_DIST_DIR" $(MAKE) -C tests/systemd-test install
	SOLSTONE_DIST_DIR="$$SOLSTONE_DIST_DIR" $(MAKE) -C tests/systemd-test legacy-upgrade
	SOLSTONE_DIST_DIR="$$SOLSTONE_DIST_DIR" $(MAKE) -C tests/systemd-test legacy-upgrade-v1022
	SOLSTONE_DIST_DIR="$$SOLSTONE_DIST_DIR" $(MAKE) -C tests/systemd-test release-crossover-v1022-deb
	SOLSTONE_DIST_DIR="$$SOLSTONE_DIST_DIR" $(MAKE) -C tests/systemd-test release-crossover-v1022-rpm

# AR_<triple>/RANLIB_<triple> must point at zig wrappers before this recipe
# invokes the producer: PATH poison covers `ar`, so cc/ffmpeg-sys-next/ort
# fallthrough must not find the shim.
check-rust-distribution-under-poison:
	@test "$$SOLSTONE_CI_POISONED" = 1 || { echo "check-rust-distribution-under-poison is internal; run 'make check-rust-distribution'" >&2; exit 2; }
	@$(REQUIRE_CARGO)
	@set -eu; \
	wrapper_dir=$$(mktemp -d); \
	trap 'rm -rf "$$wrapper_dir"' 0 1 2 15; \
	printf '%s\n' '#!/bin/sh' 'exec zig ar "$$@"' > "$$wrapper_dir/zigar"; \
	printf '%s\n' '#!/bin/sh' 'exec zig ranlib "$$@"' > "$$wrapper_dir/zigranlib"; \
	chmod 755 "$$wrapper_dir/zigar" "$$wrapper_dir/zigranlib"; \
	if [ -n "$${SOLSTONE_ZIG:-}" ]; then \
		zig_bin=$$SOLSTONE_ZIG; \
		if [ -d "$$zig_bin" ]; then zig_bin=$$zig_bin/zig; fi; \
		PATH="$$(dirname "$$zig_bin"):$$PATH"; \
		export PATH; \
	fi; \
	out_root=$${SOLSTONE_DISTRIBUTION_OUT:-/var/tmp/solstone-distribution-out}; \
	echo "producing linux-x86_64 and linux-aarch64 under poison into $$out_root"; \
	AR=$$wrapper_dir/zigar \
	RANLIB=$$wrapper_dir/zigranlib \
	AR_x86_64_unknown_linux_gnu="$$wrapper_dir/zigar" \
	RANLIB_x86_64_unknown_linux_gnu="$$wrapper_dir/zigranlib" \
	AR_aarch64_unknown_linux_gnu="$$wrapper_dir/zigar" \
	RANLIB_aarch64_unknown_linux_gnu="$$wrapper_dir/zigranlib" \
	AR_x86_64_unknown_linux_musl="$$wrapper_dir/zigar" \
	RANLIB_x86_64_unknown_linux_musl="$$wrapper_dir/zigranlib" \
	AR_aarch64_unknown_linux_musl="$$wrapper_dir/zigar" \
	RANLIB_aarch64_unknown_linux_musl="$$wrapper_dir/zigranlib" \
	$(SOLSTONE_DISTRIBUTION) produce linux-x86_64 "$$out_root/linux-x86_64"; \
	AR=$$wrapper_dir/zigar \
	RANLIB=$$wrapper_dir/zigranlib \
	AR_x86_64_unknown_linux_gnu="$$wrapper_dir/zigar" \
	RANLIB_x86_64_unknown_linux_gnu="$$wrapper_dir/zigranlib" \
	AR_aarch64_unknown_linux_gnu="$$wrapper_dir/zigar" \
	RANLIB_aarch64_unknown_linux_gnu="$$wrapper_dir/zigranlib" \
	AR_x86_64_unknown_linux_musl="$$wrapper_dir/zigar" \
	RANLIB_x86_64_unknown_linux_musl="$$wrapper_dir/zigranlib" \
	AR_aarch64_unknown_linux_musl="$$wrapper_dir/zigar" \
	RANLIB_aarch64_unknown_linux_musl="$$wrapper_dir/zigranlib" \
	$(SOLSTONE_DISTRIBUTION) produce linux-aarch64 "$$out_root/linux-aarch64"; \
	if [ -s "$${SOLSTONE_POISON_LOG:-}" ]; then \
		echo "poison log is not empty:" >&2; \
		cat "$$SOLSTONE_POISON_LOG" >&2; \
		exit 1; \
	fi; \
	echo "poison log empty"

ci-full-plan:
	@$(REQUIRE_CARGO)
	$(SOLSTONE_CI_RUNNER) plan

# Per-registry-entry wrappers preserve the native runtime contract while the
# runner keeps ownership of selection, timeout, logging, and aggregation.
define run-registry-cargo
	@set -eu; \
	test -n "$(CI_PACKAGE)" || { echo "CI_PACKAGE is required" >&2; exit 2; }; \
	run_cargo() { \
		if [ -n "$(CI_FEATURES)" ]; then \
			cargo test --manifest-path $(RUST_MANIFEST) --locked --offline "$$@" --features "$(CI_FEATURES)" -- --test-threads=1; \
		else \
			cargo test --manifest-path $(RUST_MANIFEST) --locked --offline "$$@" -- --test-threads=1; \
		fi; \
	}; \
	run_action() { $(1); }; \
	case "$(CI_RUNTIME)" in \
		none) run_action ;; \
		onnx) $(REQUIRE_ONNX_HOST_RUNTIME); $(VAD_ANALYZE_HOST_ORT_ENV) run_action ;; \
		pdf) $(REQUIRE_PDF_HOST_RUNTIME); SOLSTONE_CORE_PDF_LIBRARY="$(PDF_RUNTIME_HOST_LINK_DIR)/$(PDF_RUNTIME_HOST_LIBRARY)" run_action ;; \
		*) echo "unknown CI_RUNTIME '$(CI_RUNTIME)'" >&2; exit 2 ;; \
	esac
endef

check-rust-registry-suite:
	@test -n "$(CI_TARGET)" || { echo "CI_TARGET is required" >&2; exit 2; }
	$(call run-registry-cargo,run_cargo -p "$(CI_PACKAGE)" --test "$(CI_TARGET)")

check-rust-registry-package:
	$(call run-registry-cargo,CI_PACKAGE="$(CI_PACKAGE)" CI_FEATURES="$(CI_FEATURES)" scripts/check_rust_registry_package.sh "$(RUST_MANIFEST)")

check-rust-test:
	@$(REQUIRE_CARGO)
	cargo test --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --locked -- --test-threads=1

check-rust-describe-cli-stubs:
	@$(REQUIRE_CARGO)
	@set -eu; \
		if output="$$(cargo test --manifest-path $(RUST_MANIFEST) -p solstone-core-describe --features test-stubs --lib --test cli --locked -- --test-threads=1 2>&1)"; then \
			cargo_status=0; \
		else \
			cargo_status=$$?; \
		fi; \
		printf '%s\n' "$$output"; \
		if [ "$$cargo_status" -ne 0 ]; then \
			exit "$$cargo_status"; \
		fi; \
		if [ -n "$${SOLSTONE_CI_CARGO_LOG:-}" ]; then \
			echo "check-rust-describe-cli-stubs: recording Cargo shim active (SOLSTONE_CI_CARGO_LOG is set); the leg ran but emits no test output, so the stub census is skipped for this traversal only"; \
			exit 0; \
		fi; \
		printf '%s\n' "$$output" | grep -F '0 filtered out' >/dev/null; \
		for test_name in \
			blocking_and_session_abort_notifications_have_distinct_flat_shapes \
			blocking_or_unknown_refusals_abort_without_an_artifact \
			describe_runs_one_session_and_promotes_an_analyzed_artifact \
			describe_uses_convey_mask_for_live_handler_decode \
			detection_failures_latch_after_one_attempt \
			detection_runs_for_unselected_media_and_preserves_unfiltered_objects \
			detection_secondary_gate_uses_secondary_label \
			session_submits_a_later_request_before_the_first_response \
			tier_one_temp_is_nonempty_before_atomic_promotion_and_is_removed_afterward \
			frames_only_matches_the_frozen_oracle \
			explicit_empty_journal_uses_defaults \
			frames_only_owner_debug_and_verbose_flags_are_noops \
			malformed_invocation_is_a_usage_error \
			version_names_libavcodec \
			decode_failures_use_exit_code_two \
			launch_failure_is_blocked_not_empty; do \
			printf '%s\n' "$$output" | grep -F "test $$test_name ... ok" >/dev/null; \
		done

# Deliberately manual: this runs the supervisor integration targets repeatedly
# under bounded CPU contention. Their printed verdicts, rather
# than Make's flattened nonzero exit code, distinguish FAILED from INCONCLUSIVE.
check-rust-race: build
	@$(REQUIRE_CARGO)
	@set -u; \
	case "$(RUST_RACE_RUNS)" in ''|*[!0-9]*) echo "check-rust-race: RUST_RACE_RUNS must be a positive integer" >&2; exit 2;; esac; \
	case "$(RUST_RACE_LOAD_JOBS)" in ''|*[!0-9]*) echo "check-rust-race: RUST_RACE_LOAD_JOBS must be a positive integer" >&2; exit 2;; esac; \
	if [ "$(RUST_RACE_RUNS)" -eq 0 ] || [ "$(RUST_RACE_LOAD_JOBS)" -eq 0 ]; then \
		echo "check-rust-race: RUST_RACE_RUNS and RUST_RACE_LOAD_JOBS must be positive" >&2; exit 2; \
	fi; \
	workdir=$$(mktemp -d "$${TMPDIR:-/var/tmp}/solstone-race-XXXXXX"); \
	load_pids=''; run_pids=''; \
	cleanup() { \
		status=$$?; trap - EXIT INT TERM; \
		for pid in $$run_pids $$load_pids; do kill "$$pid" 2>/dev/null || :; done; \
		for pid in $$run_pids $$load_pids; do wait "$$pid" 2>/dev/null || :; done; \
		rm -rf "$$workdir"; \
		exit "$$status"; \
	}; \
	trap cleanup EXIT; trap 'exit 130' INT TERM; \
	classifier="$(RUST_TARGET_DIR)/debug/solstone-core-race-classifier"; \
	if [ ! -x "$$classifier" ]; then echo "check-rust-race: FAILED: classifier was not built"; exit 1; fi; \
	job=1; while [ "$$job" -le "$(RUST_RACE_LOAD_JOBS)" ]; do (while :; do :; done) & load_pids="$$load_pids $$!"; job=$$((job + 1)); done; \
	run=1; while [ "$$run" -le "$(RUST_RACE_RUNS)" ]; do \
		( cargo test --manifest-path $(RUST_MANIFEST) -p solstone-core $(RUST_RACE_TEST_TARGETS) --locked --no-fail-fast -- --test-threads=1 > "$$workdir/run-$$run.log" 2>&1; status=$$?; printf '%s\n' "$$status" > "$$workdir/run-$$run.status" ) & \
		run_pids="$$run_pids $$!"; run=$$((run + 1)); \
	done; \
	for pid in $$run_pids; do wait "$$pid" || :; done; \
	for pid in $$load_pids; do kill "$$pid" 2>/dev/null || :; done; \
	for pid in $$load_pids; do wait "$$pid" 2>/dev/null || :; done; \
	load_pids=''; run_pids=''; \
	green=0; inconclusive=0; failed=0; failed_details=''; \
	run=1; while [ "$$run" -le "$(RUST_RACE_RUNS)" ]; do \
		if [ -f "$$workdir/run-$$run.status" ]; then status=$$(cat "$$workdir/run-$$run.status"); else status=125; fi; \
		verdict=$$("$$classifier" "$$workdir/run-$$run.log" "$$status" 2>&1); classifier_status=$$?; \
		if [ "$$classifier_status" -ne 0 ]; then verdict="FAILED: classifier infrastructure: $$verdict"; fi; \
		printf '%s\n' "check-rust-race: run $$run $$verdict"; \
		case "$$verdict" in \
			GREEN) ;; \
			*) echo "check-rust-race: --- run $$run evidence ---"; \
			   sed -n '/^failures:/,$$p' "$$workdir/run-$$run.log" 2>/dev/null | head -30; \
			   grep -E "panicked at|assertion|connection closed before" "$$workdir/run-$$run.log" 2>/dev/null | head -12; \
			   echo "check-rust-race: --- end run $$run evidence ---";; \
		esac; \
		case "$$verdict" in \
			GREEN) green=$$((green + 1));; \
			INCONCLUSIVE:*) inconclusive=$$((inconclusive + 1));; \
			FAILED:*) failed=$$((failed + 1)); failed_details="$$failed_details [run $$run: $${verdict#FAILED: }]";; \
			*) failed=$$((failed + 1)); failed_details="$$failed_details [run $$run: unexpected classifier verdict $$verdict]";; \
		esac; \
		run=$$((run + 1)); \
	done; \
	if [ "$$failed" -ne 0 ]; then \
		echo "check-rust-race: FAILED ($$failed hard-failed run(s); $$inconclusive inconclusive)$$failed_details"; exit 1; \
	elif [ "$$inconclusive" -ne 0 ]; then \
		echo "check-rust-race: INCONCLUSIVE ($$inconclusive of $(RUST_RACE_RUNS) run(s); 0 hard failures)"; exit 1; \
	else \
		echo "check-rust-race: GREEN ($$green of $(RUST_RACE_RUNS) run(s))"; \
	fi

# macOS is a core platform with parity to Linux as an acceptance criterion
# This is deliberately native to the macOS SDK host:
# Linux cross-compilation could not build the workspace's Darwin C dependencies
# and the former four-package include list omitted both new crates and tests.
# `--workspace --all-targets --no-run` makes every current and future member,
# including its test targets, part of the gate by default. There are no crate
# exclusions; a source that does not compile on macOS is work, not an excuse to
# narrow the gate.
check-rust-macos:
	@set -eu; \
	if [ "$(HOST_SYSTEM)" = "Linux" ] && [ -n "$(ONNX_RUNTIME_HOST_TARGET)" ]; then \
		echo "check-rust-macos: not run on $(HOST_SYSTEM)/$(HOST_ARCH); the full-workspace gate is native to the macOS SDK host"; \
		exit 0; \
	fi; \
	$(REQUIRE_SUPPORTED_ONNX_HOST); \
	if [ "$(HOST_SYSTEM)" != "Darwin" ]; then \
		echo "unsupported host for check-rust-macos: observed $(HOST_SYSTEM)/$(HOST_ARCH); supported execution host: Darwin/arm64" >&2; \
		exit 1; \
	fi; \
	$(REQUIRE_CARGO); \
	$(REQUIRE_ONNX_HOST_RUNTIME); \
	$(VAD_ANALYZE_HOST_ORT_ENV) cargo test --manifest-path core/Cargo.toml --workspace --all-targets --no-run --locked

# Linux cross-check of the Windows Rust cfg seam. Native C/C++ roots and
# sibling-owned backends are named in a self-expiring exclusion ledger; every
# remaining library and test package is checked separately for precise residue.
check-rust-windows:
	@$(REQUIRE_CARGO)
	@$(REQUIRE_RUSTUP)
	@rustup target list --installed 2>/dev/null | grep -qx "$(WINDOWS_TARGET)" || { echo "Rust target $(WINDOWS_TARGET) is required for the Windows gate; run rustup target add $(WINDOWS_TARGET)" >&2; exit 1; }
	cargo run --manifest-path $(RUST_MANIFEST) -p solstone-core-repository-contracts --bin solstone-windows-crosscheck --locked --offline -- core/ci/windows-crosscheck.toml

# Native Windows transport. The public repo carries the exact-tree bundle,
# source binding, and native runner; the box-local bootstrap lives outside this
# repo because it contains appliance paths. This target is intentionally not a
# ci-full prerequisite: it requires a real Windows host and is operator-run.
require-win-remote-host:
	@test -n "$(WIN_REMOTE_HOST)" || { echo "WIN_REMOTE_HOST is required (user@host)" >&2; exit 1; }

sync-win-host: require-win-remote-host
	@WIN_REMOTE_HOST="$(WIN_REMOTE_HOST)" GIT="$(GIT)" SCP="$(SCP)" sh scripts/sync-win-host.sh

win-host-ci: require-win-remote-host
	@WIN_REMOTE_HOST="$(WIN_REMOTE_HOST)" GIT="$(GIT)" SCP="$(SCP)" SSH="$(SSH)" sh scripts/win-host-ci.sh

check-rust-ios:
	@$(REQUIRE_CARGO)
	@# Host-only process/server crates, including Convey, are not iOS target concerns in this wave.
	@set -eu; \
	if [ "$$(uname -s)" != "Darwin" ]; then \
		echo "check-rust-ios: not run on $$(uname -s); the Apple SDK is a native macOS-host gate"; \
	else \
		$(REQUIRE_RUSTUP); \
		command -v xcrun >/dev/null 2>&1 || { echo "xcrun is required for the iOS gate; install Xcode and retry" >&2; exit 1; }; \
		xcrun --sdk iphoneos --show-sdk-path >/dev/null || { echo "the iPhoneOS SDK is required for the iOS gate; select a complete Xcode installation and retry" >&2; exit 1; }; \
		rustup target list --installed 2>/dev/null | grep -qx "$(IOS_TARGET)" || { echo "Rust target $(IOS_TARGET) is required for the iOS gate; run rustup target add $(IOS_TARGET)" >&2; exit 1; }; \
		cargo check --manifest-path $(RUST_MANIFEST) --workspace --exclude solstone-core --exclude solstone-core-journal-cli --exclude solstone-core-indexer-store --exclude solstone-core-indexer-query --exclude solstone-core-entity --exclude solstone-core-facets --exclude solstone-core-sol-link --exclude solstone-core-spp-attest --exclude solstone-core-spp-ratls --exclude solstone-core-generate-wire --exclude solstone-core-transcribe --exclude solstone-core-convey-http --exclude solstone-core-convey-shell --exclude solstone-core-clients-web --exclude solstone-core-settings-web --exclude solstone-core-facets-web --exclude solstone-core-serving --exclude solstone-core-segment --exclude solstone-core-ingest --exclude solstone-core-entities --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-describe --exclude solstone-core-observe-audio --exclude solstone-core-body-rebuild --exclude solstone-core-vad-analyze --exclude solstone-core-convey-body --exclude solstone-core-import-host --lib --target $(IOS_TARGET) --locked; \
	fi

check-rust-deny:
	@$(REQUIRE_CARGO)
	cargo deny --manifest-path $(RUST_MANIFEST) --locked --offline check bans licenses sources

.PHONY: check-service-legacy-evidence service-legacy-evidence-capture
# Hand-run immutable-evidence regeneration. This deliberately has no `install`
# prerequisite: every leg is either stdlib Python, its own pinned interpreter
# acquisition, uv/maturin, or Cargo, and adding install would create unrelated
# journal/runtime side effects. It is intentionally not a CI prerequisite.
check-service-legacy-evidence:
	cargo fmt --manifest-path core/crates/solstone-core-service-legacy-evidence/Cargo.toml --all -- --check
	cargo clippy --manifest-path core/crates/solstone-core-service-legacy-evidence/Cargo.toml --all-targets --locked -- -D warnings
	cargo test --manifest-path core/crates/solstone-core-service-legacy-evidence/Cargo.toml --locked -- --test-threads=1

service-legacy-evidence-capture:
	@test -n "$(CAPTURE_INPUT)" || { echo "CAPTURE_INPUT=<pushed-commit> is required" >&2; exit 2; }
	python3 scripts/service_legacy_capture.py --capture-input "$(CAPTURE_INPUT)"

build:
	@$(REQUIRE_CARGO)
	cargo build --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --locked

# Build is necessary but not sufficient: these are the binaries delivered by
# maturin packaging leaves, and each must start successfully after the workspace
# build. ci_gate_purity derives the inventory from those leaves, so adding a
# package without adding its smoke command makes the Rust gate red.
check-rust-shipped-binaries: build
	@$(REQUIRE_CARGO)
	@if [ "$$(uname -s)" = "Linux" ]; then $(REQUIRE_ONNX_HOST_RUNTIME); fi
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- --version >/dev/null
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-journal-bin --bin solstone-core-journal --locked -- --version >/dev/null
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-retention-cli --bin solstone-retention --locked -- --help >/dev/null
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-sol-bin --bin solstone-core-sol --locked -- --version >/dev/null
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-describe --bin solstone-core-describe --locked -- --version >/dev/null
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-pdf --bin solstone-core-pdf --locked -- --version >/dev/null
	@set -eu; \
	if [ "$$(uname -s)" != "Linux" ]; then \
		echo "Vulkan-probe smoke: not run on $$(uname -s); this wheel ships on Linux"; \
	else \
		cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-vulkan-probe --bin solstone-core-vulkan-probe --locked -- --version >/dev/null; \
	fi
	@set -eu; \
	if output=$$(cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-depict --bin solstone-core-depict --locked 2>&1); then \
		echo "solstone-core-depict unexpectedly accepted an empty invocation" >&2; \
		exit 1; \
	else \
		run_status=$$?; \
	fi; \
	[ "$$run_status" -eq 1 ] || { echo "solstone-core-depict smoke exited $$run_status, expected usage exit 1" >&2; echo "$$output" >&2; exit 1; }; \
	case "$$output" in \
		*'"schema":"solstone-depict-error-v1"'*'"reason":"malformed-request"'*) ;; \
		*) echo "solstone-core-depict smoke did not emit its malformed-request usage record" >&2; echo "$$output" >&2; exit 1 ;; \
	esac
	@set -eu; \
	if [ "$$(uname -s)" != "Linux" ]; then \
		echo "analysis-helper smoke: not run on $$(uname -s); these wheels ship on Linux"; \
	elif output=$$($(VAD_ANALYZE_HOST_ORT_ENV) cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-speakers-analyze --bin solstone-core-speakers-analyze --locked </dev/null 2>&1); then \
		echo "solstone-core-speakers-analyze unexpectedly accepted an empty request" >&2; \
		exit 1; \
	else \
		run_status=$$?; \
	fi; \
	if [ "$$(uname -s)" = "Linux" ]; then \
		[ "$$run_status" -eq 64 ] || { echo "solstone-core-speakers-analyze smoke exited $$run_status, expected usage exit 64" >&2; echo "$$output" >&2; exit 1; }; \
		case "$$output" in \
			*'"schema":"solstone-speaker-analyze-error-v1"'*'"reason":"malformed-request"'*) ;; \
			*) echo "solstone-core-speakers-analyze did not emit its malformed-request record" >&2; echo "$$output" >&2; exit 1 ;; \
		esac; \
	fi
	@set -eu; \
	if [ "$$(uname -s)" != "Linux" ]; then \
		:; \
	elif output=$$($(VAD_ANALYZE_HOST_ORT_ENV) cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core-vad-analyze --bin solstone-core-vad-analyze --locked </dev/null 2>&1); then \
		echo "solstone-core-vad-analyze unexpectedly accepted an empty request" >&2; \
		exit 1; \
	else \
		run_status=$$?; \
	fi; \
	if [ "$$(uname -s)" = "Linux" ]; then \
		[ "$$run_status" -eq 64 ] || { echo "solstone-core-vad-analyze smoke exited $$run_status, expected usage exit 64" >&2; echo "$$output" >&2; exit 1; }; \
		case "$$output" in \
			*'"schema":"solstone-vad-error-v1"'*'"reason":"malformed-request"'*) ;; \
			*) echo "solstone-core-vad-analyze did not emit its malformed-request record" >&2; echo "$$output" >&2; exit 1 ;; \
		esac; \
	fi

.PHONY: audit check-rust-release-manifest
check-rust-release-manifest:
	@$(REQUIRE_CARGO)
	@set -eu; \
	if [ -n "$${MANIFEST:-}" ] && [ -n "$${RELEASE_DIR:-}" ]; then \
		echo "release manifest: MANIFEST and RELEASE_DIR are mutually exclusive" >&2; exit 2; \
	elif [ -n "$${MANIFEST:-}" ]; then \
		$(SOLSTONE_CI_RUNNER) release-manifest-check --manifest "$${MANIFEST}"; \
	elif [ -n "$${RELEASE_DIR:-}" ]; then \
		$(SOLSTONE_CI_RUNNER) release-manifest-check --release-dir "$${RELEASE_DIR}"; \
	else \
		$(SOLSTONE_CI_RUNNER) release-manifest-check; \
	fi

audit:
	@$(REQUIRE_CARGO)
	@set -eu; \
	[ -n "$${AUDIT_ADVISORY_BUNDLE:-}" ] || { echo "audit: AUDIT_ADVISORY_BUNDLE is required" >&2; exit 2; }; \
	[ -n "$${AUDIT_ADVISORY_RECEIPT:-}" ] || { echo "audit: AUDIT_ADVISORY_RECEIPT is required" >&2; exit 2; }; \
	[ -n "$${AUDIT_ADVISORY_PUBKEY:-}" ] || { echo "audit: AUDIT_ADVISORY_PUBKEY is required" >&2; exit 2; }; \
	[ -n "$${AUDIT_ADVISORY_LOCATOR:-}" ] || { echo "audit: AUDIT_ADVISORY_LOCATOR is required" >&2; exit 2; }; \
	$(SOLSTONE_CI_RUNNER) advisory-audit \
		--bundle "$${AUDIT_ADVISORY_BUNDLE}" \
		--receipt "$${AUDIT_ADVISORY_RECEIPT}" \
		--pubkey "$${AUDIT_ADVISORY_PUBKEY}" \
		--locator "$${AUDIT_ADVISORY_LOCATOR}"

# Re-vendor brand assets from the canonical brand source. CI verifies the
# committed output (it does not run brand-sync) — run this locally when the
# brand spec updates, then commit the diff. Pure copy: every asset below has a
# committed source in the brand tree, so no rasterizer is needed.
#
# SHELL_STATUS_SYNC copies every committed shell status file from identically
# named brand sources, including mark-connecting-animated because the connecting
# presentation references it. SHELL_STATUS_HELD is empty: nothing in this
# directory is held back from a brand-sync.
SHELL_STATUS_DIR  = core/crates/solstone-core-convey-shell/assets/static/sol-status
SHELL_STATUS_SYNC = mark:mark mark-attention:mark-attention mark-paused:mark-paused mark-offline:mark-offline mark-error:mark-error mark-connecting:mark-connecting mark-connecting-animated:mark-connecting-animated
SHELL_STATUS_HELD =

brand-sync:
	@test -n "$(BRAND_DIR)" || { echo "brand: BRAND_DIR is required — point it at your brand asset directory (BRAND_DIR=/path/to/brand make brand-sync)"; exit 1; }
	@test -d "$(BRAND_DIR)" || { echo "brand: BRAND_DIR=$(BRAND_DIR) not found"; exit 1; }
	@set -e; for pair in $(SHELL_STATUS_SYNC); do \
	  cp "$(BRAND_DIR)/$${pair#*:}.svg" "$(SHELL_STATUS_DIR)/$${pair%%:*}.svg"; \
	done
	cp "$(BRAND_DIR)/mark.svg"                    docs/static/mark.svg
	cp "$(BRAND_DIR)/png/mark-512.png"            docs/static/logo.png
	cp "$(BRAND_DIR)/web-favicon/favicon.ico"     favicon.ico
	@if [ -n "$(SHELL_STATUS_HELD)" ]; then echo "brand: held (not synced, intentionally divergent): $(SHELL_STATUS_HELD)"; fi
	@echo "brand: synced from $(BRAND_DIR)"

# Setup skill symlinks
skills:
	@$(VENV_BIN)/python scripts/build_skill_references.py
	@$(RUST_BIN)/solstone-core-sol skills install --project journal --agent all

# Start local dev stack against fixture journal (no observers, no daily processing).
# Recipe is native-only; `build` is cargo. Do not reattach `.installed`.
dev: build
	$(TEST_ENV) PATH=$(CURDIR)/$(RUST_BIN):$$PATH $(RUST_BIN)/solstone-core-journal supervisor 0 --no-daily

# Start sandbox stack: fixture copy + background supervisor + readiness wait
sandbox: build
	@# Fail if sandbox already running
	@if [ -f .sandbox.pid ] && kill -0 $$(cat .sandbox.pid) 2>/dev/null; then \
		echo "Sandbox already running (PID $$(cat .sandbox.pid))"; \
		echo "Run 'make sandbox-stop' first."; \
		exit 1; \
	fi
	@# Clean up stale state from a previous crashed sandbox
	@if [ -f .sandbox.journal ]; then \
		rm -rf "$$(cat .sandbox.journal)" 2>/dev/null; \
		rm -f .sandbox.pid .sandbox.journal; \
	fi
	@# Copy fixtures to temp dir
	@SANDBOX_JOURNAL=$$(mktemp -d /tmp/solstone-sandbox-XXXXXX); \
	cp -r tests/fixtures/journal/* "$$SANDBOX_JOURNAL/"; \
	echo "$$SANDBOX_JOURNAL" > .sandbox.journal; \
	echo "Sandbox journal: $$SANDBOX_JOURNAL"; \
	: "Boot supervisor in background"; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" SANDBOX_PATH="$(CURDIR)/$(RUST_BIN):$$PATH" SANDBOX_LOG="$$SANDBOX_JOURNAL/health/service.log" JOURNAL_BIN="$(CURDIR)/$(RUST_BIN)/solstone-core-journal" \
		$(SHELL) scripts/start_sandbox_supervisor.sh > .sandbox.pid; \
	echo "Supervisor PID: $$(cat .sandbox.pid)"; \
	: "Poll for readiness"; \
	echo "Waiting for services..."; \
	READY=false; \
	for i in $$(seq 1 20); do \
		if SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(RUST_BIN)/solstone-core-journal health > /dev/null 2>&1; then \
			READY=true; \
			break; \
		fi; \
		sleep 1; \
	done; \
	if [ "$$READY" = "false" ]; then \
		echo "Readiness timeout - killing supervisor"; \
		kill $$(cat .sandbox.pid) 2>/dev/null || true; \
		rm -rf "$$SANDBOX_JOURNAL" .sandbox.pid .sandbox.journal; \
		exit 1; \
	fi; \
	CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port" 2>/dev/null); \
	echo ""; \
	echo "Sandbox is ready!"; \
	echo "  Convey: http://localhost:$$CONVEY_PORT/"; \
	echo "  Journal: $$SANDBOX_JOURNAL"; \
	echo "  Stop:   make sandbox-stop"

# Stop sandbox: terminate supervisor, clean up temp dir and state files
sandbox-stop:
	@if [ ! -f .sandbox.pid ]; then \
		echo "No sandbox running."; \
		exit 0; \
	fi; \
	PID=$$(cat .sandbox.pid); \
	echo "Stopping supervisor (PID $$PID)..."; \
	kill "$$PID" 2>/dev/null || true; \
	: "Wait up to 5s for clean shutdown"; \
	for i in $$(seq 1 10); do \
		kill -0 "$$PID" 2>/dev/null || break; \
		sleep 0.5; \
	done; \
	kill -9 "$$PID" 2>/dev/null || true; \
	if [ -f .sandbox.journal ]; then \
		SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
		rm -rf "$$SANDBOX_JOURNAL"; \
		echo "Removed $$SANDBOX_JOURNAL"; \
	fi; \
		rm -f .sandbox.pid .sandbox.journal; \
		echo "Sandbox stopped."

.PHONY:

# Install and verify local ML models
install-models:
	@test -x "$(RUST_BIN)/solstone-core-sol" || { echo "missing $(RUST_BIN)/solstone-core-sol; run make build first" >&2; exit 1; }
	$(RUST_BIN)/solstone-core-journal install-models

# Build the parakeet helper binary (macOS/arm64 only, requires Xcode CLT)
parakeet-helper:
	cd core/crates/solstone-core-transcribe/parakeet-helper && swift build -c release
	@echo "built: $$(pwd)/core/crates/solstone-core-transcribe/parakeet-helper/.build/release/parakeet-helper"

# Remove parakeet helper build artifacts
parakeet-helper-clean:
	rm -rf core/crates/solstone-core-transcribe/parakeet-helper/.build core/crates/solstone-core-transcribe/parakeet-helper/.swiftpm core/crates/solstone-core-transcribe/parakeet-helper/Package.resolved

# Build a signed/notarized macOS Apple Silicon platform wheel
# (Darwin/arm64 only; requires Xcode CLT, Developer ID cert, and the
# `sol-pbc-notary` notarytool keychain profile in sol-signing.keychain-db).
# `uv build` runs in its own PEP 517 isolated env, so this target intentionally
# does not depend on `.installed` — the wheel build is fully decoupled from
# the dev venv install state.
ifeq ($(shell uname -s)/$(shell uname -m),Darwin/arm64)
endif

# Remove the staged helper copy under _bin/
# Test environment - use fixtures journal for all tests
TEST_ENV = SOLSTONE_JOURNAL=tests/fixtures/journal

# Venv tool shortcuts
PYTEST := $(VENV_BIN)/pytest
RUFF := $(VENV_BIN)/ruff

format-check:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all -- --check

test:
	@$(MAKE) --no-print-directory check-rust-unit
	@$(MAKE) --no-print-directory RUST_CODE_EVIDENCE_CONTEXT=test report-rust-code-evidence

check-journal-device-sim:
	python3 -m unittest discover -s tools/journal_device_sim/tests -p 'test_*.py' -v

test-cov:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

test-integration:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

test-performance:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

test-app:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

test-only:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

format:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all

# Clean build artifacts and cache files
clean:
	@$(REQUIRE_CARGO)
	@if [ "$(CLEAN_FORCE)" = "1" ]; then \
		echo "CLEAN_FORCE=1: skipping live-use census"; \
	else \
		$(CURDIR)/scripts/check_rust_target_live_use.sh "$(RUST_TARGET_DIR)"; \
	fi
	@echo "Cleaning build artifacts and cache files..."
	cargo clean --manifest-path $(RUST_MANIFEST)
	rm -rf build/ dist/ *.egg-info/
	rm -rf .pytest_cache/ .coverage
	rm -rf journal/.agents/ journal/.claude/
	find . -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
	find . -type f -name "*.pyc" -delete
	find . -type f -name "*.pyo" -delete
	find . -type f -name ".DS_Store" -delete
	rm -f .installed

# Follow installed service logs
service-logs:
	$(RUST_BIN)/solstone-core-journal service logs -f

uninstall:
	@echo "Error: 'make uninstall' is disabled. Use 'journal service uninstall', 'sol skills uninstall', and 'python -m solstone.think.install_guard uninstall' to remove installed user artifacts, or 'make clean-install' to rebuild the local dev environment." >&2
	@exit 1

FORCE:

# Clean everything and reinstall
clean-install: clean
	rm -rf $(VENV) .installed
	@echo "Error: 'make clean-install' is retired with 'make install'. Recreate a Python tooling venv with 'uv sync --group dev' only if a remaining script still needs it." >&2
	@exit 1

# Run continuous integration checks (what CI would run)
install-checks: .installed
	@echo "=== Checking formatting ==="
	@$(RUFF) format --check . || { echo "Run 'make format' to fix formatting"; exit 1; }
	@echo ""
	@echo "=== Running ruff ==="
	@$(RUFF) check . || { echo "Run 'make format' to auto-fix"; exit 1; }
	@echo ""
	@echo "=== Running layer-hygiene check ==="
	@echo ""
	@echo "=== Running API-conventions check ==="
	@$(MAKE) check-api-conventions
	@echo ""
	@echo "=== Running journal-io access check ==="
	@$(MAKE) check-journal-io-access
	@echo ""
	@echo "=== Running journal-io mechanic check ==="
	@$(MAKE) check-journal-io-mechanic
	@echo ""
	@echo "=== Running journal-config owner check ==="
	@$(MAKE) check-journal-config-owner
	@echo ""
	@echo "=== Running provider-start-command check ==="
	@$(MAKE) check-provider-start-commands
	@echo ""
	@echo "=== Running channel-adapter scrub check ==="
	@$(MAKE) check-channel-adapter-scrub
	@echo ""
	@echo "=== Running brain-health cutover check ==="
	@$(MAKE) check-brain-health-cutover
	@echo ""
	@echo "=== Running speaker-identity cutover check ==="
	@$(MAKE) check-speaker-identity-cutover
	@echo ""
	@echo "=== Running schema-bounds check ==="
	@echo ""
	@echo "=== Running rust release-manifest check ==="
	@$(MAKE) check-rust-release-manifest
	@echo ""
	@echo "=== Running SPL dependency-pin check ==="
	@$(MAKE) check-spl-dependency-pin
	@echo ""
	@echo "=== Checking conversion-wave retirements ==="
	@$(MAKE) check-conversion-retirements
	@echo "=== Checking cogitate runtime cutover ==="
	@$(MAKE) check-cogitate-cutover
	@echo "=== Checking cogitate runtime cutover coverage ==="
	@echo "=== Checking local-server argv ownership ==="
	@$(MAKE) check-local-server-argv-owner
	@echo "=== Checking local generate cutover ==="
	@$(MAKE) check-local-generate-cutover
	@echo ""
	@echo "=== Checking the raw-media release oracle ==="
	@echo ""
	@echo "=== Checking the segment-name oracle ==="
	@echo ""
	@echo "=== Checking media format parity ==="
	@echo ""
	@echo "=== Checking spl health vocabulary ==="
	@$(MAKE) check-spl-health-vocabulary
	@echo "=== Running access-imports-clean check ==="
	@echo ""
	@echo "=== Running convey-bind-imports-clean check ==="
	@echo ""
	@echo "=== Checking native sol grammar oracle ==="
	@$(MAKE) check-native-sol-grammar-oracle
	@echo ""
	@echo "=== Checking native sol root contract ==="
	@$(MAKE) check-native-sol-root-contract
	@echo ""
	@echo "=== Checking core sdist compile inputs ==="
	@echo ""
	@echo "=== Checking native sol journal-host command inventory ==="
	@$(MAKE) check-native-sol-journal-host-commands
	@echo ""
	@echo "=== Checking journal access rejection inventory ==="
	@$(MAKE) check-journal-access-rejection-inventory
	@echo ""
	@echo "=== Checking removed time parser readiness ==="
	@$(MAKE) check-removed-time-parser-ready
	@echo ""
	@echo "=== Checking native sol inventory ==="
	@$(MAKE) check-native-sol-inventory
	@echo ""
	@echo "=== Running native sol architecture check ==="
	@$(MAKE) check-native-sol-architecture
	@echo ""
	@echo "=== Checking generated skill references ==="
	@$(MAKE) check-skill-references
	@echo ""
	@echo "=== Checking native sol contract-route coverage ==="
	@echo "=== Checking import ingest door routes ==="
	@echo ""
	@echo "=== Checking native sol four-way conformance ==="
	@echo ""
	@echo "=== Checking native sol parity coverage ==="
	@$(MAKE) check-native-sol-coverage
	@echo ""
	@echo "=== Checking native sol no-python-spawn invariant ==="
	@$(MAKE) check-native-sol-no-python-spawn
	@echo ""
	@echo "=== Checking OpenAPI contract ==="
	@echo ""
	@echo "=== Checking journal format contract ==="
	@$(MAKE) check-contract
	@echo ""
	@echo "=== Checking core fixtures ==="
	@$(MAKE) check-core-fixtures
	@echo ""
	@echo "=== Checking journal resolution vectors ==="
	@echo ""
	@echo "=== Checking nvattest authority ==="
	@echo ""
	@echo "=== Running rust format check ==="
	@$(MAKE) check-rust-fmt
	@echo ""
	@echo "=== Running rust MSRV check ==="
	@$(MAKE) check-rust-msrv
	@echo ""
	@echo "=== Running rust clippy check ==="
	@$(MAKE) check-rust-clippy
	@echo ""
	@echo "=== Running rust test check ==="
	@$(MAKE) check-rust-test
	@echo ""
	@echo "=== Running rust iOS check ==="
	@$(MAKE) check-rust-ios
	@echo ""
	@echo "=== Running rust dependency policy check ==="
	@$(MAKE) check-rust-deny
	@echo ""

CI_FORBIDDEN_INTERPRETERS := python python3 pytest ruff uv
DISTRIBUTION_FORBIDDEN_TOOLS := $(CI_FORBIDDEN_INTERPRETERS) maturin pip pipx setuptools twine dpkg-deb rpmbuild ar rpm tar cpio curl wget
define run-rust-gate-under-poison
	@set -eu; \
	shim_dir=$$(mktemp -d); \
	trap 'rm -rf "$$shim_dir"' 0 1 2 15; \
	export SOLSTONE_POISON_LOG="$$shim_dir/poison.log"; \
	for interpreter in $(if $(strip $(2)),$(2),$(CI_FORBIDDEN_INTERPRETERS)); do \
		printf '%s\n' '#!/bin/sh' \
			'if [ -n "$${SOLSTONE_POISON_LOG:-}" ]; then printf "%s %s\n" "$$0" "$$*" >> "$$SOLSTONE_POISON_LOG"; fi' \
			'echo "Rust gate invoked a forbidden interpreter: $$0 $$*" >&2' \
			'exit 97' > "$$shim_dir/$$interpreter"; \
		chmod 755 "$$shim_dir/$$interpreter"; \
	done; \
	PATH="$$shim_dir:$$PATH" SOLSTONE_CI_POISONED=1 SOLSTONE_POISON_LOG="$$shim_dir/poison.log" \
		$(MAKE) CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 $(1)
endef

.PHONY: ci ci-contained ci-under-poison ci-full ci-full-under-poison
ci: ci-prep-ffmpeg
ifeq ($(HOST_SYSTEM),Linux)
	@set -eu; \
	command -v bwrap >/dev/null 2>&1 || { echo "bubblewrap is required for contained Linux CI; install bwrap and retry" >&2; exit 1; }; \
	mkdir -p "$(RUST_TARGET_DIR)"; \
	sandbox_root=$$(mktemp -d /var/tmp/solstone-ci-XXXXXX); \
	trap 'rm -rf -- "$$sandbox_root"' 0 1 2 15; \
	mkdir -p "$$sandbox_root/tmp" "$$sandbox_root/empty" "$$sandbox_root/var-tmp/home"; \
	bwrap --die-with-parent --new-session --unshare-net --unshare-pid --unshare-ipc --unshare-uts \
		--ro-bind / / --proc /proc --dev /dev \
		--bind "$$sandbox_root/tmp" /tmp --ro-bind "$$sandbox_root/empty" /run \
		--bind "$$sandbox_root/var-tmp" /var/tmp \
		--ro-bind "$(CURDIR)" "$(CURDIR)" --bind "$(RUST_TARGET_DIR)" "$(RUST_TARGET_DIR)" \
		--chdir "$(CURDIR)" --setenv HOME /var/tmp/home --setenv TMPDIR /var/tmp \
		--setenv CARGO_HOME "$(CI_CARGO_HOME)" --setenv RUSTUP_HOME "$(CI_RUSTUP_HOME)" \
		--setenv CARGO_TARGET_DIR "$(RUST_TARGET_DIR)" \
		--setenv CARGO_INCREMENTAL 0 --setenv CARGO_PROFILE_DEV_DEBUG 0 \
		--setenv CARGO_NET_OFFLINE true --setenv SOLSTONE_CI_CONTAINED 1 \
		$(MAKE) --no-print-directory ci-contained CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
else
	$(call run-rust-gate-under-poison,ci-under-poison)
endif

ci-contained:
	@test "$${SOLSTONE_CI_CONTAINED:-}" = 1 || { echo "ci-contained is internal; run 'make ci'" >&2; exit 2; }
	$(call run-rust-gate-under-poison,ci-under-poison)

ci-under-poison:
	@test "$$SOLSTONE_CI_POISONED" = 1 || { echo "ci-under-poison is internal; run 'make ci'" >&2; exit 2; }
	@$(MAKE) check-rust-fmt
	@$(MAKE) check-rust-ci-topology
	@$(MAKE) check-rust-clippy
	@$(MAKE) check-rust-unit
	@$(MAKE) --no-print-directory RUST_CODE_EVIDENCE_CONTEXT=ci report-rust-code-evidence

ci-full:
ifneq ($(strip $(HOPPER_LID)),)
	@echo "ci-full is not supported for Hopper; make ci is the only pass needed." >&2; exit 2
else
ifneq ($(strip $(SOLSTONE_CI_CLOUD)),)
ifneq ($(SOLSTONE_CI_CLOUD),1)
$(error SOLSTONE_CI_CLOUD must be exactly 1 when set)
endif
	@command -v extro-cloud-ci >/dev/null 2>&1 || { \
		echo "SOLSTONE_CI_CLOUD=1 requires extro-cloud-ci on PATH" >&2; \
		exit 2; \
	}
	extro-cloud-ci run --source "$(CURDIR)"
else
	$(call run-rust-gate-under-poison,ci-full-under-poison)
endif
endif

ci-full-under-poison:
	@test "$$SOLSTONE_CI_POISONED" = 1 || { echo "ci-full-under-poison is internal; run 'make ci-full'" >&2; exit 2; }
	$(SOLSTONE_CI_RUNNER) run

verify: ci
	@echo "Verification complete."

watch:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

coverage:
	$(call RETIRED_PYTHON_TEST_RAIL,$@)

# Update all dependencies to latest versions and refresh genai-prices
update: .installed
	@echo "Updating all dependencies to latest versions..."
	$(UV) lock -U
	$(UV) sync
	@echo "Done. All packages updated to latest."

# Update genai-prices to get latest model pricing data
# Run this when adding new models or if pricing tests fail
update-prices: .installed
	@echo "Updating genai-prices to latest version..."
	$(UV) lock -P genai-prices
	$(UV) sync
	@echo "Done. Re-run tests to verify model pricing support."

# Show installed package versions
versions: .installed
	@echo "=== Python version ==="
	$(PYTHON) --version
	@echo ""
	@echo "=== Key package versions ==="
	@$(UV) pip list | grep -E "^(pytest|ruff|Flask|numpy|Pillow|openai|anthropic)" || true

# Install pre-commit hooks (if using pre-commit)
pre-commit: .installed
	@$(UV) pip show pre-commit >/dev/null 2>&1 || { echo "Installing pre-commit..."; $(UV) pip install pre-commit; }
	$(VENV_BIN)/pre-commit install
	@echo "Pre-commit hooks installed!"

# HTTP API conventions check (see docs/CONVEY.md § HTTP API conventions)
check-api-conventions: .installed
	$(VENV_BIN)/python scripts/check_api_conventions.py

# Journal-io write-primitive access check (see AGENTS.md §7 L2)
check-journal-io-access: .installed
	$(VENV_BIN)/python scripts/check_journal_io_access.py

# Journal raw-mechanic check (see AGENTS.md §7 L2)
check-journal-io-mechanic: .installed
	$(VENV_BIN)/python scripts/check_journal_io_mechanic.py

# Journal config owner transaction boundary gate
check-journal-config-owner: .installed
	$(VENV_BIN)/python scripts/check_journal_config_owner.py

# Provider runtime start-command boundary gate
check-provider-start-commands: .installed
	$(VENV_BIN)/python scripts/check_provider_start_commands.py

# Release channel adapter scrub gate
check-channel-adapter-scrub: .installed
	$(VENV_BIN)/python scripts/check_channel_adapter_scrub.py

# Brain health cutover guard
check-brain-health-cutover: .installed
	$(VENV_BIN)/python scripts/check_brain_health_cutover.py

# Speaker-identity durable-state ownership guard
check-speaker-identity-cutover: .installed
	$(VENV_BIN)/python scripts/check_speaker_identity_cutover.py

# SPL git dependency pin guard
check-spl-dependency-pin:
	python3 scripts/check_spl_dependency_pin.py

# Conversion-wave Python and package retirement gate
check-conversion-retirements:
	python3 scripts/check_conversion_retirements.py

check-cogitate-cutover: .installed
	$(VENV_BIN)/python scripts/check_cogitate_cutover.py
	$(VENV_BIN)/python scripts/report_cogitate_cutover_coverage.py

# Local-model server argv is rendered only by solstone-core local plan.
check-local-server-argv-owner:
	python3 scripts/check_local_server_argv_owner.py

check-local-install-transport:
	python3 scripts/check_local_install_transport.py

check-local-generate-cutover:
	python3 scripts/check_local_generate_cutover.py

check-thinking-cutover:
	python3 scripts/check_thinking_cutover.py

# The spl link-health vocabulary spans two languages after the native cutover:
# the native service emits the reason codes and the callosum event name, and the
# web layer consumes them. Nothing else makes them agree, and drift is silent and
# owner-visible. This gate reads BOTH sides from source.
check-spl-health-vocabulary:
	python3 scripts/check_spl_health_vocabulary.py

# Faithful thin-base gate: build a fresh venv with the REAL base partition (no
# extras) and assert the access surface imports clean against it. Heavier than
# check-access-imports-clean (does a real install) — operator/release opt-in,
# NOT part of `make ci` (which uses the fast simulation above).
# Generated router skill references gate
check-skill-references: .installed
	$(VENV_BIN)/python scripts/build_skill_references.py --check

build-native-sol-grammar-oracle: .installed
	$(VENV_BIN)/python scripts/build_native_sol_authority_grammar.py

check-native-sol-grammar-oracle: .installed
	$(VENV_BIN)/python scripts/check_native_sol_grammar_oracle.py
	$(VENV_BIN)/python scripts/build_native_sol_authority_grammar.py --check

build-native-sol-root-contract:
	python3 scripts/build_native_sol_root_contract.py

check-native-sol-root-contract:
	python3 scripts/check_native_sol_root_contract.py

build-native-sol-journal-host-commands:
	python3 scripts/build_native_sol_journal_host_commands.py

check-native-sol-journal-host-commands:
	python3 scripts/build_native_sol_journal_host_commands.py --check

build-journal-access-rejection-inventory:
	python3 scripts/build_journal_access_rejection_inventory.py

check-journal-access-rejection-inventory:
	python3 scripts/build_journal_access_rejection_inventory.py --check

build-native-sol-inventory: .installed
	$(VENV_BIN)/python scripts/build_native_sol_inventory.py

check-native-sol-inventory: .installed
	$(VENV_BIN)/python scripts/build_native_sol_inventory.py --check

check-native-sol-architecture:
	python3 scripts/check_native_sol_architecture.py

check-native-sol-coverage: .installed
	$(VENV_BIN)/python scripts/check_native_sol_coverage.py

check-native-sol-no-python-spawn:
	python3 scripts/check_native_sol_no_python_spawn.py

check-removed-time-parser-ready:
	python3 scripts/check_removed_time_parser_ready.py

contract:
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build

# No .installed: both recipes are cargo-only now that the verb is native, and
# the prerequisite was the last thing dragging a Python venv bootstrap into a
# target that needs no interpreter.
#
# check-contract-parity is gone with the reference it compared against. It ran
# solstone.think.contract.journal beside the native builder and diffed the two
# renderings; that module was deleted with the rest of the Python tree, so the
# target had one side left and could only ever fail. Nothing invoked it -- no
# gate, no suite, no other recipe -- which is why it survived its own oracle.
check-contract:
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract check
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build --check

# category_registry.json is retired. Native describe-categories is the source;
# the Python generator wrote a file nothing reads.
core-fixtures:
	@echo "category registry is native (solstone-core-describe-categories); nothing to generate." >&2

check-core-fixtures: .installed
	$(VENV_BIN)/python scripts/check_service_runtime_reference.py
