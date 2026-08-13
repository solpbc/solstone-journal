# solstone Makefile
# Python-based AI-driven desktop journaling toolkit

# Route pytest tmp dirs to /var/tmp (disk) instead of default /tmp (tmpfs/RAM).
# Each top-level pytest invocation gets its own --basetemp so concurrent runs
# in different worktrees do not share /var/tmp/pytest-of-$USER/pytest-N/. The
# basetemp is created at recipe runtime (not parse time) and removed via shell
# trap on exit, so non-test make targets don't leak empty dirs and test runs
# don't leak full ones. PYTEST_BASETEMP_INIT must be on the same recipe shell
# line as PYTEST_BASETEMP_FLAG (each recipe line is its own shell). Do not
# re-add --basetemp to pyproject — it would pin all runs to one path and
# pytest wipes it on startup, destroying concurrent state.
export TMPDIR := /var/tmp
PYTEST_BASETEMP_INIT := BASETEMP=$$(mktemp -d /var/tmp/solstone-pytest-XXXXXX); trap 'rm -rf "$$BASETEMP"' EXIT INT TERM;
PYTEST_BASETEMP_FLAG := --basetemp "$$BASETEMP"

.PHONY: install hopper-install uninstall test test-cov test-integration test-release release-checks test-performance test-app test-only format format-check install-checks ci clean clean-install coverage watch versions update update-prices preflight pre-commit skills render-packaging check-release-package-inventory check-rust-fmt check-rust-msrv check-rust-clippy check-rust-test check-rust-race check-rust-ios check-rust-macos check-rust-deny check-rust-shipped-binaries build check-release-advisory-liveness check-rust-release-manifest check-spl-dependency-pin audit openapi check-openapi check-openapi-observer-client-contract contract check-contract journal-resolution-vectors check-journal-resolution-vectors build-native-sol-grammar-oracle check-native-sol-grammar-oracle build-native-sol-root-contract check-native-sol-root-contract check-core-sdist-compile-inputs build-native-sol-journal-host-commands check-native-sol-journal-host-commands build-journal-access-rejection-inventory check-journal-access-rejection-inventory check-native-sol-python-manifest build-native-sol-inventory check-native-sol-inventory check-native-sol-architecture check-native-sol-contract-routes check-native-sol-conformance check-native-sol-coverage check-native-sol-no-python-spawn check-native-sol-docs-links check-removed-time-parser-ready dev all sandbox sandbox-stop install-models speakers-analyze-helper parakeet-helper parakeet-helper-clean wheel-speakers-analyze-linux wheel-speakers-analyze-linux-x86_64 wheel-speakers-analyze-linux-aarch64 wheel-vad-analyze-linux wheel-vad-analyze-linux-x86_64 wheel-vad-analyze-linux-aarch64 wheel-vulkan-probe-linux wheel-vulkan-probe-linux-x86_64 wheel-vulkan-probe-linux-aarch64 wheel-pdf-linux wheel-pdf-linux-x86_64 wheel-pdf-linux-aarch64 check-rust-vad-analyze-test check-rust-onnx-stage check-rust-onnx-test check-rust-pdf-stage check-rust-pdf-test wheel-describe-linux wheel-describe-linux-x86_64 wheel-describe-linux-aarch64 wheel-macos wheel-macos-clean verify verify-api verify-schemathesis update-api-baselines eval-schemas service-logs check-layer-hygiene check-api-conventions check-journal-io-access check-journal-io-mechanic check-journal-config-owner check-call-http-only check-channel-adapter-scrub check-brain-health-cutover check-tools-http-only check-access-imports-clean check-convey-bind-imports-clean check-schema-bounds check-retention-release-oracle check-segment-name-oracle check-media-format-parity check-thin-base-install check-extras-consistency check-local-server-argv-owner check-local-install-transport check-local-generate-cutover release release-test publish-release publish-release-test check-cogitate-cutover check-cogitate-cutover-tests FORCE

# Default target - build the native workspace during the Rust-conversion freeze
all: build

# Virtual environment directory
VENV := .venv
VENV_BIN := $(VENV)/bin
VENV_PY := $(VENV_BIN)/python
PYTHON := $(VENV_PY)
RUST_MANIFEST := core/Cargo.toml
SERVICE_LEGACY_EVIDENCE_ROOT ?= core/fixtures/service_legacy_evidence
IOS_TARGET := aarch64-apple-ios
MACOS_TARGET := aarch64-apple-darwin
RUST_HOST_EXCLUDES := --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-vad-analyze

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

# Only W4b-converted supervisor tests belong here: their positive waits turn
# load-dilated exhaustion into an explicit inconclusive outcome. The three
# supervisor-domain raw-poll tests (supervisor_boot, supervisor_providers, and
# restart_convey_supervisor_seam), the two session races (cogitate_session and
# generate_session), and the two non-race tests (convey_restart_no_python_spawn
# and convey_process) remain out of scope because load could make their hard
# assertions report a false FAILED.
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
# `make wheel-describe-linux-x86_64` fails with the global export and succeeds
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
CLANG_BUILTIN_INCLUDE := $(firstword $(wildcard /usr/lib/clang/*/include))
ifneq ($(CLANG_BUILTIN_INCLUDE),)
# install builds the solstone-core wheel through maturin, and solstone-core now
# depends transitively on ffmpeg-sys-next (via solstone-core-grab), whose build
# script needs these args to find limits.h. Leaving install off this list made
# `make install` fail on a clean environment while every Rust gate stayed green,
# because the gates carry the export and check-differentials inherits it when it
# shells into install.
install .installed build check-rust-msrv check-rust-clippy check-rust-test check-rust-race check-rust-onnx-test check-rust-shipped-binaries check-differentials: export BINDGEN_EXTRA_CLANG_ARGS := -I$(CLANG_BUILTIN_INCLUDE)
endif
REQUIRE_CARGO := command -v cargo >/dev/null 2>&1 || { echo "cargo is required for Rust checks; install cargo and retry" >&2; exit 1; }
REQUIRE_RUSTUP := command -v rustup >/dev/null 2>&1 || { echo "rustup is required for the iOS gate; install rustup and retry" >&2; exit 1; }
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
# runtime directory per target — scripts/stage_speakers_analyze_runtime.py owns
# the pinned URL/digest table — and the VAD targets reuse it rather than
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
		library_path="$(ONNX_RUNTIME_HOST_LINK_DIR)/$$library"; \
		if [ ! -f "$$library_path" ] || [ ! -r "$$library_path" ]; then \
			validation_error="invalid pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual missing, non-file, or unreadable); run 'make check-rust-onnx-stage' and retry"; \
			return 10; \
		fi; \
		if digest_output=$$("$(ONNX_RUNTIME_HOST_HASH_PROGRAM)" $(ONNX_RUNTIME_HOST_HASH_ARGS) "$$library_path" 2>&1); then digest_status=0; else digest_status=$$?; fi; \
		actual_digest=$${digest_output%%[[:space:]]*}; \
		if [ "$$digest_status" -ne 0 ] || [ -z "$$actual_digest" ]; then \
			validation_error="could not checksum pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual checksum input failure); run 'make check-rust-onnx-stage' and retry"; \
			return 10; \
		fi; \
		if [ "$$actual_digest" != "$(ONNX_RUNTIME_HOST_DIGEST)" ]; then \
			validation_error="invalid pinned host ONNX Runtime file: $$library_path (expected sha256 $(ONNX_RUNTIME_HOST_DIGEST), actual $$actual_digest); run 'make check-rust-onnx-stage' and retry"; \
			return 10; \
		fi; \
	done; \
	return 0; \
}
endef

REQUIRE_ONNX_HOST_RUNTIME = $(DEFINE_ONNX_RUNTIME_VALIDATOR); if validate_onnx_runtime; then :; else validation_status=$$?; echo "$$validation_error" >&2; exit "$$validation_status"; fi
PDF_RUNTIME_HOST_TARGET := linux-$(shell uname -m)
PDF_RUNTIME_HOST_LINK_DIR := $(CURDIR)/target/pdfium-runtime-link/$(PDF_RUNTIME_HOST_TARGET)
REQUIRE_PDF_HOST_RUNTIME = test -f "$(PDF_RUNTIME_HOST_LINK_DIR)/libpdfium.so" || { echo "the pinned host PDFium runtime is required to test solstone-core-pdf; run 'make check-rust-pdf-stage' once outside make ci, then retry" >&2; exit 1; }
DESCRIBE_LINUX_X86_64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target x86_64-unknown-linux-gnu
DESCRIBE_LINUX_AARCH64_MATURIN_ARGS := --locked --zig --compatibility manylinux_2_27 --auditwheel skip --target aarch64-unknown-linux-gnu
# Derived, never written out: the helper's declared coverage lives in
# solstone/think/probe.py, which is stdlib-only precisely so it imports here
# without a venv. A literal would keep globbing the old filename if the
# measured macOS minimum ever moves.
SPEAKERS_ANALYZE_MACOS_TAG = $(shell PYTHONPATH=. python3 -c 'from solstone.think.probe import SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS as t; print(t["darwin", "arm64"])')
# Pick the GPU (CUDA) journal runtime only on x86_64 NVIDIA hosts. The
# CUDA bundle resolves onnxruntime-gpu, which ships NO aarch64 wheel on PyPI, so
# an aarch64 NVIDIA host (e.g. DGX Spark / GB10) that auto-selected `cuda` would
# die in the `.installed` `uv sync` below — before the per-arch `install` guard
# (which correctly skips non-x86_64 Linux) ever runs. Gating on x86_64 also
# keeps this coherent with the STT arch decision (aarch64-linux uses the
# parakeet.cpp CPU/Vulkan bundle). Everything non-x86_64 falls to the CPU
# `journal-cpu` group, whose onnxruntime has aarch64 wheels.
JOURNAL_VARIANT ?= $(shell if [ "$$(uname -m)" = "x86_64" ] && nvidia-smi -L >/dev/null 2>&1; then echo cuda; else echo cpu; fi)

# Dev install groups: install exactly ONE journal leaf for this host.
# `journal-cpu` and `journal-cuda` select the same journal stack and differ only
# in the ONNX runtime package, so NEVER install both and NEVER use all optional
# dependency groups:
#   - `journal-cpu` pulls onnxruntime; `journal-cuda` pulls onnxruntime-gpu. Both
#     packages own the SAME onnxruntime/ import dir and clobber each other ->
#     `import onnxruntime` fails (ModuleNotFoundError) even though uv still
#     lists it installed. Surfaces as `journal install-models` dying with "No
#     module named 'onnxruntime'".
#   - on Darwin, resolving the CUDA group also forces cuda's nvidia-* wheels,
#     which have no arm64 builds, so `uv sync` errors out outright.
# Pick the GPU group only on NVIDIA hosts; everyone else gets the CPU group.
JOURNAL_GROUP := $(if $(filter cuda,$(JOURNAL_VARIANT)),journal-cuda,journal-cpu)

# Require uv only for goals that actually use it. `preflight` is a pure
# stdlib readiness battery and `install` runs preflight as its own fail-fast
# pre-step, so neither should abort at parse time when uv is absent — they
# report uv-absence themselves. Rust-only and frozen/gated goals are likewise
# optional; Python-dependent goals outside this list still abort at parse time.
UV := $(shell command -v uv 2>/dev/null)
UV_OPTIONAL_GOALS := preflight install render-packaging check-rust-fmt check-rust-msrv check-rust-clippy check-rust-test check-rust-race check-rust-ios check-rust-macos check-rust-vad-analyze-test check-rust-onnx-stage check-rust-onnx-test check-rust-deny check-service-legacy-evidence service-legacy-evidence-capture audit ci verify test build format format-check hopper-install test-cov test-integration test-release test-performance test-app test-only watch coverage release release-test release-checks publish-release publish-release-test check-transparency-minisign publish-transparency resign-transparency-pointer
ifndef UV
ifneq ($(filter-out $(UV_OPTIONAL_GOALS),$(MAKECMDGOALS)),)
$(error uv is not installed. Install it: curl -LsSf https://astral.sh/uv/install.sh | sh)
endif
endif

# --- Rust-conversion freeze guards ------------------------------------------
# FREEZE_GUARD: the release rail and the alternate Python test rails are
# frozen for the duration of the Rust-conversion effort. There is no flag
# that restores them — the freeze lifts only when this Makefile is edited
# again. See docs/PORTING.md.
define FREEZE_GUARD
	@echo "$(1): frozen for the Rust-conversion effort (development gate is Rust-only; see docs/PORTING.md)" >&2
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
.installed: pyproject.toml packages/*/pyproject.toml uv.lock .python-version-hash .rust-core-hash
	python3 scripts/render_packaging.py
	$(MAKE) preflight
	@echo "Installing package with uv..."
	$(UV) sync --group dev --group $(JOURNAL_GROUP)
	@# Python 3.14+ needs onnxruntime from nightly (not yet on PyPI)
	@OS_NAME=$$(uname -s); \
	PY_MINOR=$$($(PYTHON) -c "import sys; print(sys.version_info.minor)"); \
	if [ "$$OS_NAME" = "Darwin" ] && [ "$$PY_MINOR" -ge 14 ]; then \
		echo "Python 3.14+ detected - installing onnxruntime from nightly feed..."; \
		$(UV) pip install --pre --no-deps --index-url https://aiinfra.pkgs.visualstudio.com/PublicPackages/_packaging/ORT-Nightly/pypi/simple/ onnxruntime; \
	fi
	@# The `--group $(JOURNAL_GROUP)` sync above already pulls the right
	@# ONNX runtime package (journal-cpu = CPU, journal-cuda = GPU).
	@$(MAKE) --no-print-directory skills
	@touch .installed

# Generate lock file if missing
uv.lock: pyproject.toml packages/*/pyproject.toml
	python3 scripts/render_packaging.py
	$(UV) lock

# Install package in editable mode with isolated venv
install: .installed
	@(cd /tmp && $(CURDIR)/$(VENV_BIN)/python -c "from solstone.think.utils import get_journal") 2>/dev/null || { \
		echo ">>> re-registering editable install"; \
		$(UV) pip install -e . --no-deps; \
		if (cd /tmp && $(CURDIR)/$(VENV_BIN)/python -c "from solstone.think.utils import get_journal"); then \
			echo ">>> re-registered successfully"; \
		else \
			echo ">>> editable install still broken; run make clean-install"; \
			exit 1; \
		fi; \
	}
	@OS_NAME=$$(uname -s); \
	ARCH=$$(uname -m); \
	if [ "$$OS_NAME" = "Darwin" ] && [ "$$ARCH" = "arm64" ]; then \
		$(MAKE) parakeet-helper || { echo 'parakeet install: helper build failed' >&2; exit 1; }; \
	elif [ "$$OS_NAME" = "Linux" ]; then \
		if [ "$$ARCH" = "x86_64" ]; then \
				echo "journal install: JOURNAL_GROUP=$(JOURNAL_GROUP)"; \
				$(UV) sync --group dev --group $(JOURNAL_GROUP) || { echo "journal install: uv sync --group dev --group $(JOURNAL_GROUP) failed" >&2; exit 1; }; \
				if [ "$(JOURNAL_VARIANT)" = "cuda" ]; then \
					$(UV) sync --group dev --group $(JOURNAL_GROUP) --reinstall-package onnxruntime-gpu || { echo "journal install: failed to force-reinstall onnxruntime-gpu" >&2; exit 1; }; \
				$(VENV_PY) -c "import onnxruntime as ort; ort.preload_dlls(cuda=True, cudnn=True); assert 'CUDAExecutionProvider' in ort.get_available_providers(), 'CUDAExecutionProvider missing after install'; print('journal install: CUDA runtime ready')" || { echo "journal install: CUDA runtime validation failed" >&2; exit 1; }; \
			fi; \
		else \
			echo "journal install: skipping unsupported Linux arch $$ARCH"; \
		fi; \
	else \
		echo "parakeet install: unsupported host '$$OS_NAME/$$ARCH'; supported: darwin/arm64, linux/x86_64" >&2; \
		exit 1; \
	fi
	@$(MAKE) speakers-analyze-helper || { echo 'speakers-analyze helper install failed' >&2; exit 1; }
	@touch .installed
	@$(VENV_BIN)/journal install-models || { echo "journal install-models failed" >&2; exit 1; }

hopper-install:
	cargo fetch --manifest-path $(RUST_MANIFEST) --locked

# Stdlib-only install-readiness battery — runs before `.venv`/`uv` exist; a
# blocker failure exits non-zero. Also wired as the first step of `.installed`.
preflight:
	python3 scripts/preflight.py

render-packaging:
	python3 scripts/render_packaging.py

wheel-speakers-analyze-linux: wheel-speakers-analyze-linux-x86_64

wheel-speakers-analyze-linux-x86_64:
	python3 scripts/stage_speakers_analyze_runtime.py --target linux-x86_64
	rm -f dist/solstone_core_speakers_analyze-*.whl
	ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(CURDIR)/target/speakers-analyze-runtime-link/linux-x86_64" MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-speakers-analyze --wheel

wheel-speakers-analyze-linux-aarch64:
	python3 scripts/stage_speakers_analyze_runtime.py --target linux-aarch64
	rm -f dist/solstone_core_speakers_analyze-*.whl
	ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(CURDIR)/target/speakers-analyze-runtime-link/linux-aarch64" MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-speakers-analyze --wheel

wheel-vad-analyze-linux: wheel-vad-analyze-linux-x86_64

wheel-vad-analyze-linux-x86_64:
	python3 scripts/stage_speakers_analyze_runtime.py --target linux-x86_64 --package-dir packages/solstone-core-vad-analyze --receipt target/vad-analyze-runtime-provenance/linux-x86_64.json
	rm -f dist/solstone_core_vad_analyze-*.whl
	ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(CURDIR)/target/speakers-analyze-runtime-link/linux-x86_64" MATURIN_PEP517_ARGS="$(VAD_ANALYZE_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-vad-analyze --wheel

wheel-vad-analyze-linux-aarch64:
	python3 scripts/stage_speakers_analyze_runtime.py --target linux-aarch64 --package-dir packages/solstone-core-vad-analyze --receipt target/vad-analyze-runtime-provenance/linux-aarch64.json
	rm -f dist/solstone_core_vad_analyze-*.whl
	ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(CURDIR)/target/speakers-analyze-runtime-link/linux-aarch64" MATURIN_PEP517_ARGS="$(VAD_ANALYZE_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-vad-analyze --wheel

wheel-pdf-linux: wheel-pdf-linux-x86_64

wheel-pdf-linux-x86_64:
	python3 scripts/stage_pdfium_runtime.py --target linux-x86_64 --package-dir packages/solstone-core-pdf --receipt target/pdfium-runtime-provenance/linux-x86_64.json
	rm -f dist/solstone_core_pdf-*.whl
	MATURIN_PEP517_ARGS="$(PDF_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-pdf --wheel

wheel-pdf-linux-aarch64:
	python3 scripts/stage_pdfium_runtime.py --target linux-aarch64 --package-dir packages/solstone-core-pdf --receipt target/pdfium-runtime-provenance/linux-aarch64.json
	rm -f dist/solstone_core_pdf-*.whl
	MATURIN_PEP517_ARGS="$(PDF_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-pdf --wheel

wheel-vulkan-probe-linux: wheel-vulkan-probe-linux-x86_64

wheel-vulkan-probe-linux-x86_64:
	rm -f dist/solstone_core_vulkan_probe-*.whl
	MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-vulkan-probe --wheel

wheel-vulkan-probe-linux-aarch64:
	rm -f dist/solstone_core_vulkan_probe-*.whl
	MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-vulkan-probe --wheel

# Staging the shared host runtime is BUILD-TIME tooling: it shells to Python, so
# it stays OUTSIDE ci/ci-under-poison, which cannot shell to an interpreter at
# all. Validate every required link and its pinned digest on every invocation;
# only invalid data invokes the existing staging operation. A healthy checkout
# is therefore a no-op, while a surviving directory can no longer hide a
# missing or corrupt library. Staging remains a single-writer operation, as it
# was before this target became a validator; validation and Cargo consumption
# are not an atomic snapshot.
check-rust-onnx-stage:
	@set -u; \
	$(REQUIRE_SUPPORTED_ONNX_HOST); \
	$(DEFINE_ONNX_RUNTIME_VALIDATOR); \
	if validate_onnx_runtime; then \
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
	if ! python3 scripts/stage_speakers_analyze_runtime.py --target $(ONNX_RUNTIME_HOST_TARGET) --package-dir packages/solstone-core-vad-analyze --receipt target/vad-analyze-runtime-provenance/$(ONNX_RUNTIME_HOST_TARGET).json; then \
		echo "failed to stage the pinned host ONNX Runtime" >&2; \
		exit 1; \
	fi; \
	if validate_onnx_runtime; then \
		echo "host ONNX Runtime staged and verified at $(ONNX_RUNTIME_HOST_LINK_DIR)"; \
	else \
		validation_status=$$?; \
		echo "$$validation_error" >&2; \
		exit "$$validation_status"; \
	fi

# Staging PDFium also shells to Python and verifies a GitHub attestation, so it
# stays OUTSIDE ci/ci-under-poison. The runtime-loaded crate itself remains in
# ordinary host Cargo selection; only its real binary tests require this stage.
$(PDF_RUNTIME_HOST_LINK_DIR):
	python3 scripts/stage_pdfium_runtime.py --target $(PDF_RUNTIME_HOST_TARGET) --package-dir packages/solstone-core-pdf --receipt target/pdfium-runtime-provenance/$(PDF_RUNTIME_HOST_TARGET).json

check-rust-pdf-stage: $(PDF_RUNTIME_HOST_LINK_DIR)
	@echo "host PDFium runtime staged at $(PDF_RUNTIME_HOST_LINK_DIR)"

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
		echo "PDFium crate tests: not run on $$(uname -s); this helper ships on Linux in W4a"; \
		exit 0; \
	fi; \
	$(REQUIRE_PDF_HOST_RUNTIME); \
	SOLSTONE_CORE_PDF_LIBRARY="$(PDF_RUNTIME_HOST_LINK_DIR)/libpdfium.so" cargo test --manifest-path $(RUST_MANIFEST) -p solstone-core-pdf --locked -- --test-threads=1

# Retained name: check-rust-shipped-binaries' recovery message named it for
# months and it is in muscle memory. It now stages AND runs all three crates.
check-rust-vad-analyze-test: check-rust-onnx-stage check-rust-onnx-test

wheel-describe-linux: wheel-describe-linux-x86_64

wheel-describe-linux-x86_64:
	rm -f dist/solstone_core_describe-*.whl
	# ffmpeg-sys-next's bindgen invocation needs Zig's target headers separately.
	ZIG_LIB_DIR="$$(zig env | sed -n 's/.*\.lib_dir = "\([^"]*\)".*/\1/p')"; \
	BINDGEN_EXTRA_CLANG_ARGS="-nostdinc --target=x86_64-unknown-linux-gnu -isystem $$ZIG_LIB_DIR/include -isystem $$ZIG_LIB_DIR/libc/include/x86-linux-gnu -isystem $$ZIG_LIB_DIR/libc/include/generic-glibc -isystem $$ZIG_LIB_DIR/libc/include/x86-linux-any -isystem $$ZIG_LIB_DIR/libc/include/any-linux-any"; \
	export BINDGEN_EXTRA_CLANG_ARGS; \
	# FFmpeg's configure sees this native Rust target as host; force its C probe to the wheel baseline.
	cc="zig cc -target x86_64-linux-gnu.2.27 -I$(CURDIR)/core/crates/solstone-core-describe/build-support/zig-glibc" MATURIN_PEP517_ARGS="$(DESCRIBE_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-describe --wheel

wheel-describe-linux-aarch64:
	rm -f dist/solstone_core_describe-*.whl
	MATURIN_PEP517_ARGS="$(DESCRIBE_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-describe --wheel

check-rust-fmt:
	@$(REQUIRE_CARGO)
	cargo fmt --manifest-path $(RUST_MANIFEST) --all -- --check

check-rust-msrv:
	@$(REQUIRE_CARGO)
	@$(REQUIRE_RUSTUP)
	@rustup toolchain list 2>/dev/null | grep -Eq '^1\.95\.0(-|[[:space:]])' || { echo "Rust toolchain 1.95.0 is required for the MSRV gate; run rustup toolchain install 1.95.0" >&2; exit 1; }
	RUSTUP_TOOLCHAIN=1.95.0 cargo check --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --locked

check-rust-clippy:
	@$(REQUIRE_CARGO)
	cargo clippy --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --all-targets --locked -- -D warnings

check-rust-test:
	@$(REQUIRE_CARGO)
	cargo test --manifest-path $(RUST_MANIFEST) --workspace $(RUST_HOST_EXCLUDES) --locked -- --test-threads=1

# Deliberately manual: this runs the W4b-converted supervisor integration
# targets repeatedly under bounded CPU contention. Its printed verdicts, rather
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
	classifier="$(CURDIR)/core/target/debug/solstone-core-race-classifier"; \
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
# (founder, 2026-08-10). This is deliberately native to the macOS SDK host:
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
	command -v rustup >/dev/null 2>&1 || { echo "rustup is required for the macOS gate; install rustup and retry" >&2; exit 1; }; \
	installed_targets=$$(mktemp "$${TMPDIR:-/var/tmp}/solstone-rustup-targets-XXXXXX"); \
	trap 'rm -f "$$installed_targets"' EXIT INT TERM; \
	if ! rustup target list --installed > "$$installed_targets" 2>&1; then \
		echo "rustup failed to inspect installed targets for the macOS gate" >&2; \
		exit 1; \
	fi; \
	grep -qx "aarch64-apple-darwin" "$$installed_targets" || { echo "Rust target aarch64-apple-darwin is required for the macOS gate; run rustup target add aarch64-apple-darwin" >&2; exit 1; }; \
	$(REQUIRE_ONNX_HOST_RUNTIME); \
	$(VAD_ANALYZE_HOST_ORT_ENV) cargo test --manifest-path core/Cargo.toml --workspace --all-targets --no-run --target aarch64-apple-darwin --locked

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
		cargo check --manifest-path $(RUST_MANIFEST) --workspace --exclude solstone-core --exclude solstone-core-journal-cli --exclude solstone-core-indexer-store --exclude solstone-core-indexer-query --exclude solstone-core-entity --exclude solstone-core-facets --exclude solstone-core-sol-link --exclude solstone-core-spp-attest --exclude solstone-core-spp-ratls --exclude solstone-core-generate-wire --exclude solstone-core-transcribe --exclude solstone-core-convey-http --exclude solstone-core-convey-shell --exclude solstone-core-serving --exclude solstone-core-segment --exclude solstone-core-ingest --exclude solstone-core-entities --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-describe --exclude solstone-core-observe-audio --exclude solstone-core-body-rebuild --exclude solstone-core-vad-analyze --lib --target $(IOS_TARGET) --locked; \
	fi

check-rust-deny:
	@$(REQUIRE_CARGO)
	cargo fetch --manifest-path $(RUST_MANIFEST) --locked
	cargo deny --manifest-path $(RUST_MANIFEST) --locked --offline check bans licenses sources

# Cross-language differentials: Rust tests whose oracle is the running Python
# implementation. They need a populated .venv, so they carry
# `required-features = ["differential"]` in their crate manifests, which gates
# them off `make ci` and is what lets that gate run on a bare checkout. This
# target installs first, then runs exactly those tests.
# ci_gate_purity::every_differential_test_is_named_in_its_own_gate asserts that
# every differential target in the workspace is named here, so a differential
# cannot be gated off `make ci` and then run nowhere.
# A red leg must never hide a leg that never ran. Two mechanisms did exactly
# that here, and closing either one alone leaves the other: `make` halts a
# recipe at its first failing line, so a red package hid every later package;
# and `cargo` halts after the first failing test *target* within one
# invocation, so `wire` -- 14 tests, the one-shot conformance suite -- stopped
# running the moment `session_real` went red. Every leg therefore runs with
# --no-fail-fast, every leg runs regardless of its predecessors, and the target
# exits non-zero if any of them did. A gate that could not judge must never
# read as a gate that said yes.
# The final leg is the only one that links ONNX Runtime, so it -- and only it --
# carries the ORT_* / LD_LIBRARY_PATH plumbing and the staged-runtime
# prerequisite. It sits outside the loop precisely so those variables do not
# leak into legs that must not see them.
# The Vulkan differential fails without a loader unless the operator explicitly
# sets SOLSTONE_VULKAN_DIFFERENTIAL_NO_LOADER=1; its --nocapture mode makes the
# resulting RUN or SKIP report visible in this gate's output.
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

.PHONY: check-differentials
check-differentials: check-rust-onnx-stage build
	@$(REQUIRE_CARGO)
	$(MAKE) install
	@status=0; \
	echo "==> cargo clippy --features differential -p solstone-core --test native_sol_coverage"; \
	cargo clippy --manifest-path $(RUST_MANIFEST) --features differential --locked --no-deps \
		-p solstone-core --test native_sol_coverage -- -D warnings || status=$$?; \
	for leg in \
		"-p solstone-core --test native_sol_coverage --test journal_config_client --test journal_config_corruption --test body_restore_client --test grab_differential" \
		"-p solstone-core-journal-bin --test journal_process_bootstrap" \
		"-p solstone-core-generate-wire --test responsiveness_differential --test token_log_differential" \
		"-p solstone-core-spp-attest --test spp_attest_differential" \
		"-p solstone-core-spp-ratls --test composite_differential" \
		"-p solstone-core-local --test admission_cross_process --test vulkan_differential --test local_fit_report_differential --test install_provider_differential --test downloading_installers_differential -- --nocapture" \
		"-p solstone-core-observer --test observer_list_json_differential --test observer_status_differential --test observer_list_human_differential --test observer_reconcile_dry_run_differential --test observer_increment_stat_differential --test observer_resolve_identity_differential --test observer_prune_dry_run_differential" \
		"-p solstone-core-system --test stt_backend_choice_differential --test partition_differential" \
		"-p solstone-core-callosum --test callosum_cross_process --test registry_conformance" \
		"-p solstone-core-transfer --test transfer_differential" \
		"-p solstone-core --test transfer_send_differential --test export_differential" \
		"-p solstone-core-system-health --test pipeline_health_oracle" \
		"-p solstone-core-observe-audio --test audio_differential" \
		"-p solstone-core-transcribe --test transcribe_differential" \
		"-p solstone-core-transcribe --test transcribe_sound_tags_differential" \
		"-p solstone-core-import-sources --test archive_merge_oracle" ; do \
		echo "==> cargo test --features differential --no-fail-fast $$leg"; \
		cargo test --manifest-path $(RUST_MANIFEST) --features differential --locked --no-fail-fast $$leg \
			|| status=$$?; \
	done; \
	ort_leg="-p solstone-core-vad-analyze --test vad_differential"; \
	echo "==> cargo test --features differential --no-fail-fast $$ort_leg"; \
	$(VAD_ANALYZE_HOST_ORT_ENV) \
		cargo test --manifest-path $(RUST_MANIFEST) --features differential --locked --no-fail-fast $$ort_leg \
			|| status=$$?; \
	transcribe_ort_leg="-p solstone-core-transcribe --test transcribe_vad_differential"; \
	echo "==> cargo test --features differential --no-fail-fast $$transcribe_ort_leg"; \
	$(VAD_ANALYZE_HOST_ORT_ENV) \
		cargo test --manifest-path $(RUST_MANIFEST) --features differential --locked --no-fail-fast $$transcribe_ort_leg \
			|| status=$$?; \
	if [ $$status -eq 0 ]; then \
		echo "check-differentials: every leg ran and passed"; \
	else \
		echo "check-differentials: FAILED (status $$status) -- every leg above still ran; read each leg's own result line"; \
	fi; \
	exit $$status

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

audit:
	@$(REQUIRE_CARGO)
	@python3 scripts/check_release_preflight.py cargo-deny >&2
	@python3 scripts/advisory_mirror_audit.py --bundle "$(AUDIT_ADVISORY_BUNDLE)" --receipt "$(AUDIT_ADVISORY_RECEIPT)" --pubkey "$(AUDIT_ADVISORY_PUBKEY)" --locator "$(AUDIT_ADVISORY_LOCATOR)"

# Setup skill symlinks
skills:
	@$(VENV_BIN)/python scripts/build_skill_references.py
	@$(VENV_BIN)/sol skills install --project journal --agent all

# Start local dev stack against fixture journal (no observers, no daily processing)
dev: .installed
	$(TEST_ENV) PATH=$(CURDIR)/$(VENV_BIN):$$PATH $(VENV_BIN)/journal supervisor 0 --no-daily

# Start sandbox stack: fixture copy + background supervisor + readiness wait
sandbox: .installed
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
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" SANDBOX_PATH="$(CURDIR)/$(VENV_BIN):$$PATH" SANDBOX_LOG="$$SANDBOX_JOURNAL/health/service.log" JOURNAL_BIN="$(CURDIR)/$(VENV_BIN)/journal" \
		$(VENV_PY) scripts/start_sandbox_supervisor.py > .sandbox.pid; \
	echo "Supervisor PID: $$(cat .sandbox.pid)"; \
	: "Poll for readiness"; \
	echo "Waiting for services..."; \
	READY=false; \
	for i in $$(seq 1 20); do \
		if SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/journal health > /dev/null 2>&1; then \
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

.PHONY: sandbox-seed-observers
sandbox-seed-observers: ## Seed 4 sample observers into the running sandbox journal
	@test -s .sandbox.journal || (echo "No sandbox running. Run 'make sandbox' first." && exit 1)
	@SOLSTONE_JOURNAL=$$(cat .sandbox.journal) $(VENV_BIN)/python tests/fixtures/seed_observers.py

# Verify API baselines against running sandbox
verify-api: .installed
	@echo "Verifying API baselines (sandbox)..."
	@$(MAKE) sandbox
	@SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
	CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port"); \
	RESULT=0; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/journal indexer --rescan-full > /dev/null; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/python tests/verify_api.py verify --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
	$(MAKE) sandbox-stop; \
	exit $$RESULT

# tests/conftest.py overwrites SOLSTONE_JOURNAL; pass sandbox via a private env var.
verify-schemathesis: .installed ## Run Schemathesis read allowlist against disposable live sandbox
	@echo "Verifying OpenAPI contract with Schemathesis (disposable live sandbox)..."
	@$(MAKE) sandbox
	@SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
	RESULT=0; \
	SOLSTONE_SCHEMATHESIS_JOURNAL="$$SANDBOX_JOURNAL" \
	SOLSTONE_SCHEMATHESIS_LIVE=1 \
	$(VENV_BIN)/pytest tests/test_openapi_schemathesis.py -q || RESULT=$$?; \
	$(MAKE) sandbox-stop; \
	exit $$RESULT

eval-schemas: .installed
	$(VENV_BIN)/python tests/eval_schemas.py

# Regenerate API baseline files. By default uses the deterministic Flask
# test-client path (frozen time). For sandbox-only endpoints (graph, search,
# badge-count, updated-days), pass SANDBOX=1 to regenerate from the live
# sandbox — these rely on the indexer and real clock.
update-api-baselines: .installed
	@if [ "$(SANDBOX)" = "1" ]; then \
		echo "Updating API baselines (sandbox, includes sandbox-only endpoints)..."; \
		$(MAKE) sandbox; \
		SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
		CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port"); \
		RESULT=0; \
		SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/journal indexer --rescan-full > /dev/null; \
		SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/python tests/verify_api.py update --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
		$(MAKE) sandbox-stop; \
		exit $$RESULT; \
	else \
		echo "Updating API baselines (test client)..."; \
		$(VENV_BIN)/python tests/verify_api.py update; \
	fi


# Install and verify local ML models
install-models:
	@test -x "$(VENV_BIN)/sol" || { echo "missing $(VENV_BIN)/sol; run make install first" >&2; exit 1; }
	$(VENV_BIN)/journal install-models

speakers-analyze-helper:
	@test -x "$(VENV_BIN)/python" || { echo "missing $(VENV_BIN)/python; run make install first" >&2; exit 1; }
	@$(VENV_BIN)/python scripts/install_speakers_analyze_helper.py

# Build the parakeet helper binary (macOS/arm64 only, requires Xcode CLT)
parakeet-helper:
	cd solstone/observe/transcribe/parakeet_helper && swift build -c release
	@echo "built: $$(pwd)/solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper"

# Remove parakeet helper build artifacts
parakeet-helper-clean:
	rm -rf solstone/observe/transcribe/parakeet_helper/.build solstone/observe/transcribe/parakeet_helper/.swiftpm solstone/observe/transcribe/parakeet_helper/Package.resolved

# Build a signed/notarized macOS Apple Silicon platform wheel
# (Darwin/arm64 only; requires Xcode CLT, Developer ID cert, and the
# `sol-pbc-notary` notarytool keychain profile in sol-signing.keychain-db).
# `uv build` runs in its own PEP 517 isolated env, so this target intentionally
# does not depend on `.installed` — the wheel build is fully decoupled from
# the dev venv install state.
ifeq ($(shell uname -s)/$(shell uname -m),Darwin/arm64)
wheel-macos: parakeet-helper
	@ROOT_FACTS=$$(mktemp); \
	trap 'rm -f "$$ROOT_FACTS"' EXIT; \
	echo "==> signing and notarizing parakeet-helper"; \
	./scripts/sign-and-notarize-helper.sh solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper > "$$ROOT_FACTS"; \
	echo "==> staging helper into _bin/"; \
	mkdir -p solstone/observe/transcribe/parakeet_helper/_bin; \
	cp solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper solstone/observe/transcribe/parakeet_helper/_bin/parakeet-helper; \
	echo "==> building macosx_14_0_arm64 platform wheel"; \
	rm -rf build/ *.egg-info/; \
	$(UV) build --wheel -C--build-option=--plat-name=macosx_14_0_arm64; \
	ROOT_MAC_WHEEL=$$(ls dist/solstone-*-macosx_14_0_arm64.whl); \
	SOURCE_COMMIT=$$(git rev-parse HEAD); \
	CORE_LOCK_SHA256=$$(shasum -a 256 core/Cargo.lock | awk '{print $$1}'); \
	python3 -m scripts.record_macos_native_wheel --role root --wheel "$$ROOT_MAC_WHEEL" --signing-facts "$$ROOT_FACTS" --source-commit "$$SOURCE_COMMIT" --core-lock-sha256 "$$CORE_LOCK_SHA256" --out dist/macos-native-root.json
	@echo "==> building, signing, and recording declared macOS native packages"
	python3 scripts/build_macos_release_packages.py
else
wheel-macos:
	@echo "wheel-macos: only supported on Darwin/arm64 (got $(shell uname -s)/$(shell uname -m))" >&2
	@exit 1
endif

# Remove the staged helper copy that wheel-macos installs into _bin/
wheel-macos-clean:
	rm -rf solstone/observe/transcribe/parakeet_helper/_bin

# Test environment - use fixtures journal for all tests
TEST_ENV = SOLSTONE_JOURNAL=tests/fixtures/journal

# Venv tool shortcuts
PYTEST := $(VENV_BIN)/pytest
RUFF := $(VENV_BIN)/ruff

format-check:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all -- --check

test: check-rust-test

test-cov:
	$(call FREEZE_GUARD,$@)

test-integration:
	$(call FREEZE_GUARD,$@)

test-release:
	$(call FREEZE_GUARD,$@)

release-checks:
	$(call FREEZE_GUARD,$@)

test-performance:
	$(call FREEZE_GUARD,$@)

test-app:
	$(call FREEZE_GUARD,$@)

test-only:
	$(call FREEZE_GUARD,$@)

format:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all

# Clean build artifacts and cache files
clean:
	@echo "Cleaning build artifacts and cache files..."
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
	$(VENV_BIN)/journal service logs -f

uninstall:
	@echo "Error: 'make uninstall' is disabled. Use 'journal service uninstall', 'sol skills uninstall', and 'python -m solstone.think.install_guard uninstall' to remove installed user artifacts, or 'make clean-install' to rebuild the local dev environment." >&2
	@exit 1

FORCE:

# Clean everything and reinstall
clean-install: clean
	rm -rf $(VENV) .installed
	$(MAKE) install

# Run continuous integration checks (what CI would run)
install-checks: .installed
	@echo "=== Checking formatting ==="
	@$(RUFF) format --check . || { echo "Run 'make format' to fix formatting"; exit 1; }
	@echo ""
	@echo "=== Running ruff ==="
	@$(RUFF) check . || { echo "Run 'make format' to auto-fix"; exit 1; }
	@echo ""
	@echo "=== Running layer-hygiene check ==="
	@$(MAKE) check-layer-hygiene
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
	@echo "=== Running provider-install owner check ==="
	@$(MAKE) check-provider-install-owner
	@echo ""
	@echo "=== Running provider-start-command check ==="
	@$(MAKE) check-provider-start-commands
	@echo "=== Running legacy-chat surface check ==="
	@$(MAKE) check-no-legacy-chat
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
	@$(MAKE) check-schema-bounds
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
	@$(MAKE) check-cogitate-cutover-tests
	@echo "=== Checking local-server argv ownership ==="
	@$(MAKE) check-local-server-argv-owner
	@echo "=== Checking local generate cutover ==="
	@$(MAKE) check-local-generate-cutover
	@echo ""
	@echo "=== Checking the raw-media release oracle ==="
	@$(MAKE) check-retention-release-oracle
	@echo ""
	@echo "=== Checking the segment-name oracle ==="
	@$(MAKE) check-segment-name-oracle
	@echo ""
	@echo "=== Checking media format parity ==="
	@$(MAKE) check-media-format-parity
	@echo ""
	@echo "=== Checking spl health vocabulary ==="
	@$(MAKE) check-spl-health-vocabulary
	@echo "=== Running access-imports-clean check ==="
	@$(MAKE) check-access-imports-clean
	@echo ""
	@echo "=== Running convey-bind-imports-clean check ==="
	@$(MAKE) check-convey-bind-imports-clean
	@echo ""
	@echo "=== Checking native sol grammar oracle ==="
	@$(MAKE) check-native-sol-grammar-oracle
	@echo ""
	@echo "=== Checking native sol root contract ==="
	@$(MAKE) check-native-sol-root-contract
	@echo ""
	@echo "=== Checking core sdist compile inputs ==="
	@$(MAKE) check-core-sdist-compile-inputs
	@echo ""
	@echo "=== Checking native sol journal-host command inventory ==="
	@$(MAKE) check-native-sol-journal-host-commands
	@echo ""
	@echo "=== Checking journal access rejection inventory ==="
	@$(MAKE) check-journal-access-rejection-inventory
	@echo ""
	@echo "=== Checking native sol docs links ==="
	@$(MAKE) check-native-sol-docs-links
	@echo ""
	@echo "=== Checking removed time parser readiness ==="
	@$(MAKE) check-removed-time-parser-ready
	@echo ""
	@echo "=== Checking native sol Python manifest ==="
	@$(MAKE) check-native-sol-python-manifest
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
	@$(MAKE) check-native-sol-contract-routes
	@echo ""
	@echo "=== Checking native sol four-way conformance ==="
	@$(MAKE) check-native-sol-conformance
	@echo ""
	@echo "=== Checking native sol parity coverage ==="
	@$(MAKE) check-native-sol-coverage
	@echo ""
	@echo "=== Checking native sol no-python-spawn invariant ==="
	@$(MAKE) check-native-sol-no-python-spawn
	@echo ""
	@echo "=== Checking OpenAPI contract ==="
	@$(MAKE) check-openapi
	@echo ""
	@echo "=== Checking journal format contract ==="
	@$(MAKE) check-contract
	@echo ""
	@echo "=== Checking core fixtures ==="
	@$(MAKE) check-core-fixtures
	@echo ""
	@echo "=== Checking packaging render ==="
	@python3 scripts/render_packaging.py --check
	@echo ""
	@echo "=== Checking journal resolution vectors ==="
	@$(MAKE) check-journal-resolution-vectors
	@echo ""
	@echo "=== Checking nvattest authority ==="
	@$(MAKE) check-nvattest-authority
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
	@echo "=== Checking extras consistency ==="
	@$(VENV_BIN)/python scripts/check_extras_consistency.py
	@echo ""
	@echo "=== Checking release package inventory ==="
	@python3 scripts/release_package_inventory.py
	@echo ""

check-release-package-inventory:
	@python3 scripts/release_package_inventory.py

# check-release-package-inventory is deliberately NOT here: it shells to python3.
# It already runs in install-checks, which depends on .installed and is where
# interpreter-requiring checks belong. Run it directly with
# `make check-release-package-inventory`.
CI_FORBIDDEN_INTERPRETERS := python python3 pytest ruff uv
.PHONY: ci ci-under-poison
ci:
	@set -eu; \
	shim_dir=$$(mktemp -d); \
	trap 'rm -rf "$$shim_dir"' 0 1 2 15; \
	for interpreter in $(CI_FORBIDDEN_INTERPRETERS); do \
		printf '%s\n' '#!/bin/sh' 'echo "make ci invoked a forbidden interpreter: $$0 $$*" >&2' 'exit 97' > "$$shim_dir/$$interpreter"; \
		chmod 755 "$$shim_dir/$$interpreter"; \
	done; \
	PATH="$$shim_dir:$$PATH" SOLSTONE_CI_POISONED=1 $(MAKE) ci-under-poison

ci-under-poison:
	@test "$$SOLSTONE_CI_POISONED" = 1 || { echo "ci-under-poison is internal; run 'make ci'" >&2; exit 2; }
	@$(MAKE) check-rust-fmt
	@$(MAKE) check-rust-msrv
	@$(MAKE) check-rust-clippy
	@$(MAKE) check-rust-test
	@$(MAKE) check-rust-onnx-test
	@$(MAKE) check-rust-pdf-test
	@$(MAKE) check-rust-shipped-binaries
	@$(MAKE) check-rust-ios
	@$(MAKE) check-rust-macos
	@$(MAKE) check-rust-deny
	@echo "All CI checks passed (Rust-only; Rust-conversion freeze in effect — see docs/PORTING.md)"
	@echo "Not run here: the cross-language differentials, which need a Python install. Run 'make check-differentials' when you touch a seam both languages implement; run 'make check-rust-race' for concurrency-sensitive supervisor changes."

verify: ci
	@echo "Verification complete! (alias for ci during the Rust-conversion freeze)"

watch:
	$(call FREEZE_GUARD,$@)

coverage:
	$(call FREEZE_GUARD,$@)

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

# Low-bar layer-hygiene check (see docs/coding-standards.md § Layer Hygiene)
check-layer-hygiene: .installed
	$(VENV_BIN)/python scripts/check_layer_hygiene.py

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

# Provider install ownership boundary gate
check-provider-install-owner: .installed
	$(VENV_BIN)/python scripts/check_provider_install_owner.py

# Provider runtime start-command boundary gate
check-provider-start-commands: .installed
	$(VENV_BIN)/python scripts/check_provider_start_commands.py

# Removed chat surfaces stay out of tracked Python, HTML, and JavaScript.
check-no-legacy-chat: .installed
	$(VENV_BIN)/python scripts/check_no_legacy_chat.py

# Release channel adapter scrub gate
check-channel-adapter-scrub: .installed
	$(VENV_BIN)/python scripts/check_channel_adapter_scrub.py

# Brain health cutover guard
check-brain-health-cutover: .installed
	$(VENV_BIN)/python scripts/check_brain_health_cutover.py

# Speaker-identity durable-state ownership guard
check-speaker-identity-cutover: .installed
	$(VENV_BIN)/python scripts/check_speaker_identity_cutover.py

# Generation schema bounds ratchet
check-schema-bounds: .installed
	$(VENV_BIN)/python scripts/check_schema_bounds.py

# Rust release-manifest schema, semantic, determinism, and transaction gate
check-rust-release-manifest: .installed
	$(VENV_BIN)/python scripts/check_rust_release_manifest.py

# SPL git dependency pin guard
check-spl-dependency-pin:
	python3 scripts/check_spl_dependency_pin.py

# Conversion-wave Python and package retirement gate
check-conversion-retirements:
	python3 scripts/check_conversion_retirements.py

check-cogitate-cutover: .installed
	$(VENV_BIN)/python scripts/check_cogitate_cutover.py
	$(VENV_BIN)/python scripts/report_cogitate_cutover_coverage.py

check-cogitate-cutover-tests: .installed
	$(VENV_BIN)/python -m pytest tests/test_check_cogitate_cutover.py tests/test_cogitate_runtime_fully_retired.py tests/test_talents.py tests/test_talent_provenance.py tests/test_cogitate_client.py tests/test_cortex.py tests/test_provider_validation.py tests/test_talent.py tests/test_talent_cli.py tests/test_brain_cli.py

# Local-model server argv is rendered only by solstone-core local plan.
check-local-server-argv-owner:
	python3 scripts/check_local_server_argv_owner.py

check-local-install-transport:
	python3 scripts/check_local_install_transport.py

check-local-generate-cutover:
	python3 scripts/check_local_generate_cutover.py

# Two readers decide irreversibly about the owner's raw media and they do not
# apply the same predicate. This regenerates the oracle from the reference and
# fails if the committed fixture has drifted -- or if any row records a port
# RELEASING raw media the reference held, which is a loosening of an
# irreversible path and needs its own argument.
check-retention-release-oracle: .installed
	$(VENV_BIN)/python scripts/retention_release_oracle.py --check

# Two languages independently classify a segment directory by its NAME, and both
# scan for the key pattern rather than matching the whole name -- so a trailing
# decoration leaves a directory classified as a segment under the undecorated
# key, and the key is not the directory name. Those verdicts decide where owner
# media may be moved to during an irreversible removal, and a cross-language
# disagreement arms at cutover. This gate executes BOTH references.
check-segment-name-oracle: .installed
	$(VENV_BIN)/python scripts/segment_name_oracle.py --check

# Two implementations classify media by extension, and retention's release
# predicate rests on the split: audio and video route to a handler whose record can
# prove the raw was consumed, and an image routes to NO handler, which is what holds
# an image's original forever. Drift stops a whole format working and no test in
# either language notices. This gate reads BOTH tables.
check-media-format-parity: .installed
	$(VENV_BIN)/python scripts/media_format_parity.py

# The spl link-health vocabulary spans two languages after the native cutover:
# the native service emits the reason codes and the callosum event name, and the
# web layer consumes them. Nothing else makes them agree, and drift is silent and
# owner-visible. This gate reads BOTH sides from source.
check-spl-health-vocabulary:
	python3 scripts/check_spl_health_vocabulary.py

# Package dependency and script ownership consistency gate
check-extras-consistency: .installed
	$(VENV_BIN)/python scripts/check_extras_consistency.py

# Thin sol access surface import-clean gate (fast meta_path simulation; in ci)
check-access-imports-clean: .installed
	$(VENV_BIN)/python scripts/check_access_imports_clean.py

# Convey bind path import-clean gate
check-convey-bind-imports-clean: .installed
	$(VENV_BIN)/python scripts/check_convey_bind_imports_clean.py

# Faithful thin-base gate: build a fresh venv with the REAL base partition (no
# extras) and assert the access surface imports clean against it. Heavier than
# check-access-imports-clean (does a real install) — operator/release opt-in,
# NOT part of `make ci` (which uses the fast simulation above).
check-thin-base-install:
	python3 scripts/check_access_imports_clean.py --real-install

# Generated router skill references gate
check-skill-references: .installed
	$(VENV_BIN)/python scripts/build_skill_references.py --check

openapi:
	$(VENV_BIN)/python scripts/build_openapi_contract.py

check-openapi: .installed
	$(VENV_BIN)/python scripts/check_openapi_contract.py
	$(VENV_BIN)/python scripts/build_openapi_contract.py --check
	$(MAKE) check-openapi-observer-client-contract

check-openapi-observer-client-contract: .installed
	$(VENV_BIN)/python scripts/check_observer_client_contract_bundle.py

journal-resolution-vectors:
	$(VENV_BIN)/python scripts/build_journal_resolution_vectors.py

check-journal-resolution-vectors: .installed
	$(VENV_BIN)/python scripts/build_journal_resolution_vectors.py --check

nvattest-authority:
	$(VENV_BIN)/python scripts/build_nvattest_authority.py

nvattest-payload-facts:
	$(VENV_BIN)/python scripts/build_nvattest_payload_facts.py

check-nvattest-authority: .installed
	$(VENV_BIN)/python scripts/build_nvattest_authority.py --check

build-native-sol-grammar-oracle: .installed
	$(VENV_BIN)/python scripts/build_native_sol_authority_grammar.py

check-native-sol-grammar-oracle: .installed
	$(VENV_BIN)/python scripts/check_native_sol_grammar_oracle.py
	$(VENV_BIN)/python scripts/build_native_sol_authority_grammar.py --check

build-native-sol-root-contract:
	python3 scripts/build_native_sol_root_contract.py

check-native-sol-root-contract:
	python3 scripts/check_native_sol_root_contract.py

check-core-sdist-compile-inputs:
	python3 scripts/check_core_sdist_compile_inputs.py

build-native-sol-journal-host-commands:
	python3 scripts/build_native_sol_journal_host_commands.py

check-native-sol-journal-host-commands:
	python3 scripts/build_native_sol_journal_host_commands.py --check

build-journal-access-rejection-inventory:
	python3 scripts/build_journal_access_rejection_inventory.py

check-journal-access-rejection-inventory:
	python3 scripts/build_journal_access_rejection_inventory.py --check

check-native-sol-python-manifest:
	python3 scripts/check_native_sol_python_manifest.py

build-native-sol-inventory: .installed
	$(VENV_BIN)/python scripts/build_native_sol_inventory.py

check-native-sol-inventory: .installed
	$(VENV_BIN)/python scripts/build_native_sol_inventory.py --check

check-native-sol-architecture:
	python3 scripts/check_native_sol_architecture.py

check-native-sol-contract-routes: .installed
	$(VENV_BIN)/python scripts/check_native_sol_contract_routes.py

check-native-sol-conformance: .installed
	$(VENV_BIN)/python scripts/check_native_sol_conformance.py

check-native-sol-coverage: .installed
	$(VENV_BIN)/python scripts/check_native_sol_coverage.py

check-native-sol-no-python-spawn:
	python3 scripts/check_native_sol_no_python_spawn.py

check-native-sol-docs-links:
	python3 scripts/check_native_sol_docs_links.py

check-removed-time-parser-ready:
	python3 scripts/check_removed_time_parser_ready.py

contract:
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build

# No .installed: both recipes are cargo-only now that the verb is native, and
# the prerequisite was the last thing dragging a Python venv bootstrap into a
# target that needs no interpreter. check-contract-parity keeps it -- that one
# really does run the reference.
check-contract:
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract check
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build --check

check-contract-parity: .installed
	@set -eu; scratch=$$(mktemp -d); trap 'rm -rf "$$scratch"' EXIT; cp -R solstone "$$scratch/solstone"; \
	$(VENV_BIN)/python -c 'from pathlib import Path; import sys; from solstone.think.contract.journal import build_bundle, render_bundle_json; print(render_bundle_json(build_bundle(Path(sys.argv[1]))), end="")' "$$scratch" > "$$scratch/python.json"; \
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build --root "$$scratch" >/dev/null; \
	cmp "$$scratch/python.json" "$$scratch/solstone/talent/journal/contract/bundle.json"

core-fixtures:
	$(VENV_BIN)/python scripts/generate_observe_category_registry.py
	$(VENV_BIN)/python scripts/build_core_fixtures.py

check-core-fixtures: .installed
	$(VENV_BIN)/python scripts/generate_observe_category_registry.py --check
	$(VENV_BIN)/python scripts/build_core_fixtures.py --check

check-release-advisory-liveness: .installed
	$(VENV_BIN)/python scripts/check_release_advisory_liveness.py

# Operator-opt-in install-state smoke: drives the real install primitives
# (real uv Popen for bundled providers, real httpx for local llama-server +
# GGUF download, real huggingface_hub for MLX snapshot) against a tmp
# journal_config and asserts canonical phase transitions, byte-count
# surfacing, and post-restart state persistence. Hits the same code paths
# the dashboard hits, end-to-end. Heavier than `make test` because it does
# real network fetches; lighter than `make smoke-cogitate` because it does
# not require API keys or a running supervisor.
smoke-install-providers: .installed
	@echo "Running install-state integration smoke..."
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) \
	  solstone/apps/settings/tests/test_providers_payload_extended.py \
	  -v --tb=short --timeout=120

release: ## Locked publication entrypoint
	$(call FREEZE_GUARD,$@)

release-test: ## Locked test-publication entrypoint
	$(call FREEZE_GUARD,$@)

.PHONY: check-transparency-minisign
check-transparency-minisign:
ifeq ($(TRANSPARENCY_ACTIVATED),1)
	@$(MAKE) .installed
	$(VENV_BIN)/python scripts/check_transparency_minisign.py
else
	$(call TRANSPARENCY_GUARD,$@)
endif

.PHONY: publish-release publish-release-test publish-transparency resign-transparency-pointer
publish-release:
	$(call FREEZE_GUARD,$@)

publish-release-test:
	$(call FREEZE_GUARD,$@)

publish-transparency:
ifeq ($(TRANSPARENCY_ACTIVATED),1)
	@test -n "$(RELEASE_DIR)" || { echo "publish-transparency: set RELEASE_DIR=<retained ready dir>" >&2; exit 1; }
	@$(MAKE) .installed
	RELEASE_DIR="$(RELEASE_DIR)" SOURCE_COMMIT="$(SOURCE_COMMIT)" $(VENV_BIN)/python scripts/transparency_publish.py publish --root .
else
	$(call TRANSPARENCY_GUARD,$@)
endif

resign-transparency-pointer:
ifeq ($(TRANSPARENCY_ACTIVATED),1)
	@$(MAKE) .installed
	$(VENV_BIN)/python scripts/transparency_publish.py resign-transparency-pointer --root .
else
	$(call TRANSPARENCY_GUARD,$@)
endif
