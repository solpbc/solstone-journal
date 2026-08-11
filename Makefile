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

.PHONY: install hopper-install uninstall test test-cov test-integration test-release release-checks test-performance test-app test-only format format-check install-checks ci clean clean-install coverage watch versions update update-prices preflight pre-commit skills render-packaging check-release-package-inventory check-rust-fmt check-rust-msrv check-rust-clippy check-rust-test check-rust-ios check-rust-macos check-rust-deny check-rust-shipped-binaries build check-release-advisory-liveness check-rust-release-manifest check-spl-dependency-pin audit openapi check-openapi check-openapi-observer-client-contract contract check-contract journal-resolution-vectors check-journal-resolution-vectors build-native-sol-grammar-oracle check-native-sol-grammar-oracle build-native-sol-root-contract check-native-sol-root-contract check-core-sdist-compile-inputs build-native-sol-journal-host-commands check-native-sol-journal-host-commands build-journal-access-rejection-inventory check-journal-access-rejection-inventory check-native-sol-python-manifest build-native-sol-inventory check-native-sol-inventory check-native-sol-architecture check-native-sol-contract-routes check-native-sol-conformance check-native-sol-coverage check-native-sol-no-python-spawn check-native-sol-docs-links check-removed-time-parser-ready dev all sandbox sandbox-stop install-models speakers-analyze-helper parakeet-helper parakeet-helper-clean wheel-speakers-analyze-linux wheel-speakers-analyze-linux-x86_64 wheel-speakers-analyze-linux-aarch64 wheel-vad-analyze-linux wheel-vad-analyze-linux-x86_64 wheel-vad-analyze-linux-aarch64 wheel-vulkan-probe-linux wheel-vulkan-probe-linux-x86_64 wheel-vulkan-probe-linux-aarch64 check-rust-vad-analyze-test check-rust-onnx-stage check-rust-onnx-test wheel-describe-linux wheel-describe-linux-x86_64 wheel-describe-linux-aarch64 wheel-macos wheel-macos-clean verify verify-api verify-schemathesis update-api-baselines eval-schemas service-logs check-layer-hygiene check-api-conventions check-journal-io-access check-journal-io-mechanic check-journal-config-owner check-call-http-only check-channel-adapter-scrub check-brain-health-cutover check-tools-http-only check-access-imports-clean check-convey-bind-imports-clean check-schema-bounds check-retention-release-oracle check-segment-name-oracle check-media-format-parity check-thin-base-install check-extras-consistency check-cogitate-prompts check-local-server-argv-owner check-local-install-transport check-local-generate-cutover smoke-cogitate release release-test publish-release publish-release-test FORCE

# Default target - build the native workspace during the Rust-conversion freeze
all: build

# Virtual environment directory
VENV := .venv
VENV_BIN := $(VENV)/bin
VENV_PY := $(VENV_BIN)/python
PYTHON := $(VENV_PY)
RUST_MANIFEST := core/Cargo.toml
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
CLANG_BUILTIN_INCLUDE := $(firstword $(wildcard /usr/lib/clang/*/include))
ifneq ($(CLANG_BUILTIN_INCLUDE),)
# install builds the solstone-core wheel through maturin, and solstone-core now
# depends transitively on ffmpeg-sys-next (via solstone-core-grab), whose build
# script needs these args to find limits.h. Leaving install off this list made
# `make install` fail on a clean environment while every Rust gate stayed green,
# because the gates carry the export and check-differentials inherits it when it
# shells into install.
install .installed build check-rust-msrv check-rust-clippy check-rust-test check-rust-shipped-binaries check-differentials: export BINDGEN_EXTRA_CLANG_ARGS := -I$(CLANG_BUILTIN_INCLUDE)
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
# Host link inputs for the ONNX-bundling helpers. There is exactly ONE staged
# runtime directory per target — scripts/stage_speakers_analyze_runtime.py owns
# the pinned URL/digest table — and the VAD targets reuse it rather than
# provisioning a second copy. Only the wheel *payload* is per-package, because
# each helper's build.rs rpath points at its own $ORIGIN/../lib/<package>.
ONNX_RUNTIME_HOST_TARGET := linux-$(shell uname -m)
ONNX_RUNTIME_HOST_LINK_DIR := $(CURDIR)/target/speakers-analyze-runtime-link/$(ONNX_RUNTIME_HOST_TARGET)
VAD_ANALYZE_HOST_ORT_ENV := ORT_PREFER_DYNAMIC_LINK=true ORT_LIB_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)" LD_LIBRARY_PATH="$(ONNX_RUNTIME_HOST_LINK_DIR)$${LD_LIBRARY_PATH:+:$$LD_LIBRARY_PATH}"
REQUIRE_ONNX_HOST_RUNTIME = test -f "$(ONNX_RUNTIME_HOST_LINK_DIR)/libonnxruntime.so.1" || { echo "the pinned host ONNX Runtime is required to build and test the shipped analysis helpers; run 'make check-rust-onnx-stage' once outside make ci, then retry" >&2; exit 1; }
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
UV_OPTIONAL_GOALS := preflight install render-packaging check-rust-fmt check-rust-msrv check-rust-clippy check-rust-test check-rust-ios check-rust-vad-analyze-test check-rust-onnx-stage check-rust-onnx-test check-rust-deny audit ci verify test build format format-check hopper-install test-cov test-integration test-release test-performance test-app test-only watch coverage release release-test release-checks publish-release publish-release-test check-transparency-minisign publish-transparency resign-transparency-pointer
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

wheel-vulkan-probe-linux: wheel-vulkan-probe-linux-x86_64

wheel-vulkan-probe-linux-x86_64:
	rm -f dist/solstone_core_vulkan_probe-*.whl
	MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_X86_64_MATURIN_ARGS)" $(UV) build --package solstone-core-vulkan-probe --wheel

wheel-vulkan-probe-linux-aarch64:
	rm -f dist/solstone_core_vulkan_probe-*.whl
	MATURIN_PEP517_ARGS="$(SPEAKERS_ANALYZE_LINUX_AARCH64_MATURIN_ARGS)" $(UV) build --package solstone-core-vulkan-probe --wheel

# Host-side staged runtime for the ONNX-linked crates' own cargo test runs.
# Directory-existence rule, so a populated stage is not re-downloaded; the wheel
# targets above always restage explicitly for their own cross target.
$(ONNX_RUNTIME_HOST_LINK_DIR):
	python3 scripts/stage_speakers_analyze_runtime.py --target $(ONNX_RUNTIME_HOST_TARGET) --package-dir packages/solstone-core-vad-analyze --receipt target/vad-analyze-runtime-provenance/$(ONNX_RUNTIME_HOST_TARGET).json

# Staging the shared host runtime is BUILD-TIME tooling: it shells to Python, so
# it stays OUTSIDE ci/ci-under-poison, which cannot shell to an interpreter at
# all. Run it once per checkout; the rule is directory-existence, so a populated
# stage is not re-downloaded.
check-rust-onnx-stage: $(ONNX_RUNTIME_HOST_LINK_DIR)
	@echo "host ONNX Runtime staged at $(ONNX_RUNTIME_HOST_LINK_DIR)"

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

# macOS is a core platform with parity to Linux as an acceptance criterion
# (founder, 2026-08-10), and until this target existed NOTHING compiled the
# `#[cfg(target_os = "macos")]` paths: check-rust-ios targets aarch64-apple-ios,
# which does not build macos cfg blocks, and no other gate names a darwin target.
# Scoped to the crates that actually carry macOS code and cross-compile from
# Linux; solstone-core-body-ingest is excluded because ring and libsqlite3-sys
# need a darwin C toolchain, which is an environment limit rather than a defect.
check-rust-macos:
	@$(REQUIRE_CARGO)
	@$(REQUIRE_RUSTUP)
	@rustup target list --installed 2>/dev/null | grep -qx "$(MACOS_TARGET)" || { echo "Rust target $(MACOS_TARGET) is required for the macOS gate; run rustup target add $(MACOS_TARGET)" >&2; exit 1; }
	cargo check --manifest-path $(RUST_MANIFEST) -p solstone-core-system -p solstone-core-local -p solstone-core-journal-io --lib --target $(MACOS_TARGET) --locked

check-rust-ios:
	@$(REQUIRE_CARGO)
	@set -eu; \
	if [ "$$(uname -s)" != "Darwin" ]; then \
		echo "check-rust-ios: not run on $$(uname -s); the Apple SDK is a native macOS-host gate"; \
	else \
		$(REQUIRE_RUSTUP); \
		command -v xcrun >/dev/null 2>&1 || { echo "xcrun is required for the iOS gate; install Xcode and retry" >&2; exit 1; }; \
		xcrun --sdk iphoneos --show-sdk-path >/dev/null || { echo "the iPhoneOS SDK is required for the iOS gate; select a complete Xcode installation and retry" >&2; exit 1; }; \
		rustup target list --installed 2>/dev/null | grep -qx "$(IOS_TARGET)" || { echo "Rust target $(IOS_TARGET) is required for the iOS gate; run rustup target add $(IOS_TARGET)" >&2; exit 1; }; \
		# Host-only process/server crates, including Convey, are not iOS target concerns in this wave. \
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
.PHONY: check-differentials
check-differentials: $(ONNX_RUNTIME_HOST_LINK_DIR) build
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
		"-p solstone-core-local --test admission_cross_process --test vulkan_differential -- --nocapture" \
		"-p solstone-core-observer --test observer_list_json_differential --test observer_status_differential --test observer_list_human_differential --test observer_reconcile_dry_run_differential --test observer_increment_stat_differential --test observer_resolve_identity_differential --test observer_prune_dry_run_differential" \
		"-p solstone-core-system --test stt_backend_choice_differential --test partition_differential" \
		"-p solstone-core-callosum --test callosum_cross_process --test registry_conformance" \
		"-p solstone-core-transfer --test transfer_differential" \
		"-p solstone-core --test transfer_send_differential --test export_differential" \
		"-p solstone-core-system-health --test pipeline_health_oracle" \
		"-p solstone-core-observe-audio --test audio_differential" \
		"-p solstone-core-transcribe --test transcribe_differential" \
		"-p solstone-core-transcribe --test transcribe_sound_tags_differential" ; do \
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
	@echo "=== Running cogitate-prompt check ==="
	@$(MAKE) check-cogitate-prompts
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
	@$(MAKE) check-rust-shipped-binaries
	@$(MAKE) check-rust-ios
	@$(MAKE) check-rust-macos
	@$(MAKE) check-rust-deny
	@echo "All CI checks passed (Rust-only; Rust-conversion freeze in effect — see docs/PORTING.md)"
	@echo "Not run here: the cross-language differentials, which need a Python install. Run 'make check-differentials' when you touch a seam both languages implement."

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

# Cogitate-prompt static gate (prompts use only on-contract command forms)
check-cogitate-prompts: .installed
	$(VENV_BIN)/python scripts/check_cogitate_prompts.py

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

check-contract: .installed
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract check
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build --check

check-contract-parity: .installed
	@set -eu; scratch=$$(mktemp -d); trap 'rm -rf "$$scratch"' EXIT; cp -R solstone "$$scratch/solstone"; \
	$(VENV_BIN)/python -c 'from pathlib import Path; import sys; from solstone.think.contract.journal import build_bundle, render_bundle_json; print(render_bundle_json(build_bundle(Path(sys.argv[1]))), end="")' "$$scratch" > "$$scratch/python.json"; \
	cargo run --quiet --manifest-path $(RUST_MANIFEST) -p solstone-core --bin solstone-core --locked -- contract build --root "$$scratch" >/dev/null; \
	cmp "$$scratch/python.json" "$$scratch/solstone/talent/journal/contract/bundle.json"

core-fixtures:
	$(VENV_BIN)/python scripts/build_core_fixtures.py

check-core-fixtures: .installed
	$(VENV_BIN)/python scripts/build_core_fixtures.py --check

check-release-advisory-liveness: .installed
	$(VENV_BIN)/python scripts/check_release_advisory_liveness.py

# Re-run the live four-backend integrated-façade cogitate smoke. Spawns an
# external runner script against this venv so the real openhands-sdk Agent path
# is exercised end-to-end. Requires real API keys in env (`ANTHROPIC_API_KEY`,
# `OPENAI_API_KEY`, `GOOGLE_API_KEY`) and `llama-server` on PATH for the `local`
# backend. Catches v1.23-style Agent schema regressions that the openhands-fake
# unit tests cannot. Set COGITATE_SMOKE_RUNNER=/path/to/script to point at the
# runner; there is no default.
COGITATE_SMOKE_RUNNER ?=

smoke-cogitate: .installed
	@test -f "$(COGITATE_SMOKE_RUNNER)" || { echo "cogitate smoke runner not found: $(COGITATE_SMOKE_RUNNER)" >&2; echo "set COGITATE_SMOKE_RUNNER=/path/to/script to override" >&2; exit 1; }
	$(VENV_PY) "$(COGITATE_SMOKE_RUNNER)"

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
