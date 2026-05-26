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

.PHONY: install uninstall test test-cov test-apps test-app test-only test-integration test-integration-only test-all format format-check install-checks ci clean clean-install coverage watch versions update update-prices pre-commit skills dev all sandbox sandbox-stop install-pinchtab install-models parakeet-helper parakeet-helper-clean wheel-macos wheel-macos-clean verify-browser update-browser-baselines review verify verify-api update-api-baselines service-logs check-layer-hygiene smoke-cogitate release release-test FORCE

# Default target - install package in editable mode
all: install

# Virtual environment directory
VENV := .venv
VENV_BIN := $(VENV)/bin
VENV_PY := $(VENV_BIN)/python
PYTHON := $(VENV_PY)
PARAKEET_ONNX_VARIANT ?= $(shell if nvidia-smi -L >/dev/null 2>&1; then echo cuda; else echo cpu; fi)

# Dev install extras: Darwin lacks arm64 wheels for parakeet-onnx-cuda's
# nvidia-* deps, so on Darwin we sync only the platform-agnostic extras and
# skip the full extras sync (which would otherwise force resolution of
# parakeet variants and fail). All other hosts (Linux primary) keep it.
ifeq ($(shell uname -s),Darwin)
EXTRAS_ARGS := --extra pdf --extra whisper
else
EXTRAS_ARGS := --all-extras
endif

# Require uv
UV := $(shell command -v uv 2>/dev/null)
ifndef UV
$(error uv is not installed. Install it: curl -LsSf https://astral.sh/uv/install.sh | sh)
endif

# User bin directory for symlink (standard location, usually already in PATH)
USER_BIN := $(HOME)/.local/bin

.python-version-hash: FORCE
	@tmp_file=$$(mktemp); \
	python3 -c "import sys; print(sys.version_info[:2])" > "$$tmp_file"; \
	if [ ! -f $@ ] || ! cmp -s "$$tmp_file" $@; then mv "$$tmp_file" $@; else rm -f "$$tmp_file"; fi

# Marker file to track installation
.installed: pyproject.toml uv.lock .python-version-hash
	@echo "Installing package with uv..."
	$(UV) sync --group dev $(EXTRAS_ARGS)
	@echo "Installing Playwright Chromium browser..."
	$(VENV_BIN)/python -m playwright install chromium
	@# Python 3.14+ needs onnxruntime from nightly (not yet on PyPI)
	@OS_NAME=$$(uname -s); \
	PY_MINOR=$$($(PYTHON) -c "import sys; print(sys.version_info.minor)"); \
	if [ "$$OS_NAME" = "Darwin" ] && [ "$$PY_MINOR" -ge 14 ]; then \
		echo "Python 3.14+ detected - installing onnxruntime from nightly feed..."; \
		$(UV) pip install --pre --no-deps --index-url https://aiinfra.pkgs.visualstudio.com/PublicPackages/_packaging/ORT-Nightly/pypi/simple/ onnxruntime; \
	fi
	@OS_NAME=$$(uname -s); \
	ARCH=$$(uname -m); \
	if [ "$$OS_NAME" = "Linux" ] && [ "$$ARCH" = "x86_64" ]; then \
		echo "parakeet install: PARAKEET_ONNX_VARIANT=$(PARAKEET_ONNX_VARIANT)"; \
		$(UV) sync --extra all --extra parakeet-onnx-$(PARAKEET_ONNX_VARIANT) || { echo "parakeet install: uv sync --extra all --extra parakeet-onnx-$(PARAKEET_ONNX_VARIANT) failed" >&2; exit 1; }; \
	fi
	@$(MAKE) --no-print-directory skills
	@touch .installed

# Generate lock file if missing
uv.lock: pyproject.toml
	$(UV) lock

# Install package in editable mode with isolated venv
install: .installed
	@(cd /tmp && $(CURDIR)/$(VENV_BIN)/python -c "from solstone.think.sol_cli import main") 2>/dev/null || { \
		echo ">>> re-registering editable install"; \
		$(UV) pip install -e . --no-deps; \
		if (cd /tmp && $(CURDIR)/$(VENV_BIN)/python -c "from solstone.think.sol_cli import main"); then \
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
			echo "parakeet install: PARAKEET_ONNX_VARIANT=$(PARAKEET_ONNX_VARIANT)"; \
			$(UV) sync --extra all --extra parakeet-onnx-$(PARAKEET_ONNX_VARIANT) || { echo "parakeet install: uv sync --extra all --extra parakeet-onnx-$(PARAKEET_ONNX_VARIANT) failed" >&2; exit 1; }; \
			if [ "$(PARAKEET_ONNX_VARIANT)" = "cuda" ]; then \
				$(UV) pip install --reinstall onnxruntime-gpu || { echo "parakeet install: failed to force-reinstall onnxruntime-gpu" >&2; exit 1; }; \
				$(VENV_PY) -c "import onnxruntime as ort; ort.preload_dlls(cuda=True, cudnn=True); assert 'CUDAExecutionProvider' in ort.get_available_providers(), 'CUDAExecutionProvider missing after install'; print('parakeet install: CUDA runtime ready')" || { echo "parakeet install: CUDA runtime validation failed" >&2; exit 1; }; \
			fi; \
		else \
			echo "parakeet install: skipping unsupported Linux arch $$ARCH"; \
		fi; \
	else \
		echo "parakeet install: unsupported host '$$OS_NAME/$$ARCH'; supported: darwin/arm64, linux/x86_64" >&2; \
		exit 1; \
	fi
	@touch .installed
	@$(VENV_BIN)/journal install-models || { echo "journal install-models failed" >&2; exit 1; }

# Setup skill symlinks
skills:
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
	# Boot supervisor in background \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" PATH=$(CURDIR)/$(VENV_BIN):$$PATH \
		$(VENV_BIN)/journal supervisor 0 --no-daily \
		> "$$SANDBOX_JOURNAL/health/supervisor.log" 2>&1 & \
	echo $$! > .sandbox.pid; \
	echo "Supervisor PID: $$(cat .sandbox.pid)"; \
	# Poll for readiness \
	echo "Waiting for services..."; \
	READY=false; \
	for i in $$(seq 1 20); do \
		if SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/sol health > /dev/null 2>&1; then \
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
	# Wait up to 5s for clean shutdown \
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
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/sol indexer --rescan-full > /dev/null; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/python tests/verify_api.py verify --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
	$(MAKE) sandbox-stop; \
	exit $$RESULT

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
		SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/sol indexer --rescan-full > /dev/null; \
		SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/python tests/verify_api.py update --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
		$(MAKE) sandbox-stop; \
		exit $$RESULT; \
	else \
		echo "Updating API baselines (test client)..."; \
		$(VENV_BIN)/python tests/verify_api.py update; \
	fi


# Install pinchtab browser automation tool
install-pinchtab:
	@if command -v pinchtab >/dev/null 2>&1; then \
		echo "pinchtab already installed: $$(pinchtab --version 2>/dev/null || echo 'unknown')"; \
	else \
		echo "Installing pinchtab..."; \
		curl -fsSL https://pinchtab.com/install.sh | bash; \
	fi

# Install and verify local ML models
install-models:
	@test -x "$(VENV_BIN)/sol" || { echo "missing $(VENV_BIN)/sol; run make install first" >&2; exit 1; }
	$(VENV_BIN)/journal install-models

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
	@echo "==> signing and notarizing parakeet-helper"
	./scripts/sign-and-notarize-helper.sh solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper
	@echo "==> staging helper into _bin/"
	mkdir -p solstone/observe/transcribe/parakeet_helper/_bin
	cp solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper solstone/observe/transcribe/parakeet_helper/_bin/parakeet-helper
	@echo "==> building macosx_14_0_arm64 platform wheel"
	$(UV) build --wheel -C--build-option=--plat-name=macosx_14_0_arm64
else
wheel-macos:
	@echo "wheel-macos: only supported on Darwin/arm64 (got $(shell uname -s)/$(shell uname -m))" >&2
	@exit 1
endif

# Remove the staged helper copy that wheel-macos installs into _bin/
wheel-macos-clean:
	rm -rf solstone/observe/transcribe/parakeet_helper/_bin

# Run browser scenarios against sandbox
verify-browser: .installed
	@echo "Running browser scenarios (sandbox)..."
	@$(MAKE) sandbox
	@SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
	CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port"); \
	RESULT=0; \
	$(VENV_BIN)/python tests/verify_browser.py verify --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
	$(MAKE) sandbox-stop; \
	exit $$RESULT

# Re-capture all browser baseline screenshots
update-browser-baselines: .installed
	@echo "Updating browser baselines (sandbox)..."
	@$(MAKE) sandbox
	@SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
	CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port"); \
	RESULT=0; \
	$(VENV_BIN)/python tests/verify_browser.py update --base-url "http://localhost:$$CONVEY_PORT" || RESULT=$$?; \
	$(MAKE) sandbox-stop; \
	exit $$RESULT

# Full product verification: API baselines + browser scenarios
review: .installed
	@command -v pinchtab >/dev/null 2>&1 || { \
		echo "pinchtab is required for browser verification."; \
		echo "Run 'make install-pinchtab' to install it."; \
		exit 1; \
	}
	@echo "=== Starting review ==="
	@$(MAKE) sandbox
	@SANDBOX_JOURNAL=$$(cat .sandbox.journal); \
	CONVEY_PORT=$$(cat "$$SANDBOX_JOURNAL/health/convey.port"); \
	BASE_URL="http://localhost:$$CONVEY_PORT"; \
	RESULT_API=0; \
	RESULT_BROWSER=0; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/sol indexer --rescan-full > /dev/null; \
	echo ""; \
	echo "=== API baseline verification ==="; \
	SOLSTONE_JOURNAL="$$SANDBOX_JOURNAL" $(VENV_BIN)/python tests/verify_api.py verify --base-url "$$BASE_URL" || RESULT_API=$$?; \
	echo ""; \
	echo "=== Browser scenario verification ==="; \
	$(VENV_BIN)/python tests/verify_browser.py verify --base-url "$$BASE_URL" || RESULT_BROWSER=$$?; \
	echo ""; \
	echo "=== Stopping sandbox ==="; \
	$(MAKE) sandbox-stop; \
	echo ""; \
	echo "=== Review Summary ==="; \
	if [ $$RESULT_API -eq 0 ]; then \
		echo "  API:     PASS"; \
	else \
		echo "  API:     FAIL"; \
	fi; \
	if [ $$RESULT_BROWSER -eq 0 ]; then \
		echo "  Browser: PASS"; \
	else \
		echo "  Browser: FAIL"; \
	fi; \
	echo ""; \
	if [ $$RESULT_API -eq 0 ] && [ $$RESULT_BROWSER -eq 0 ]; then \
		echo "Review: ALL PASS"; \
	else \
		echo "Review: FAIL"; \
		exit 1; \
	fi

# Test environment - use fixtures journal for all tests
TEST_ENV = SOLSTONE_JOURNAL=tests/fixtures/journal
# Marker-based exclusion: anything decorated `pytest.mark.integration` is held
# out of `make test`. New live-network tests need only the marker — no Makefile edit.
NOT_INTEGRATION = -m "not integration"

# Venv tool shortcuts
PYTEST := $(VENV_BIN)/pytest
RUFF := $(VENV_BIN)/ruff
MYPY := $(VENV_BIN)/mypy

# Check formatting without modifying files — gates `make test`
format-check: .installed
	@$(RUFF) format --check . || { echo "Run 'make format' to fix formatting"; exit 1; }

# Run core tests (excluding integration and app tests)
# -n auto --dist loadgroup lives here, not in pyproject addopts, so bare
# pytest / pytest-watch / IDE runs stay serial. The root conftest
# workerinput controller-guard keeps direct `pytest -n auto` correct too.
test: .installed format-check
	@echo "Running core tests..."
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) tests/ -q --ignore=tests/integration $(NOT_INTEGRATION) -n auto --dist loadgroup

# Run core tests with full-repo coverage (used by ci/verify)
test-cov: .installed format-check
	@echo "Running core tests with coverage..."
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) tests/ -q --cov=. --ignore=tests/integration $(NOT_INTEGRATION) -n auto --dist loadgroup
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) solstone/apps/link/tests/test_workspace_qr_size.py -q --cov=. --cov-append

# Run app tests
test-apps: .installed
	@echo "Running app tests..."
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) solstone/apps/ -q

# Run specific app tests
test-app: .installed
	@if [ -z "$(APP)" ]; then \
		echo "Usage: make test-app APP=<app_name>"; \
		echo "Example: make test-app APP=todos"; \
		exit 1; \
	fi
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) solstone/apps/$(APP)/tests/ -v

# Run specific test file or pattern
test-only: .installed
	@if [ -z "$(TEST)" ]; then \
		echo "Usage: make test-only TEST=<test_file_or_pattern>"; \
		echo "Example: make test-only TEST=tests/test_utils.py"; \
		echo "Example: make test-only TEST=\"-k test_function_name\""; \
		exit 1; \
	fi
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) $(TEST)

# Run integration tests
test-integration: .installed
	@echo "Running integration tests..."
	@$(PYTEST_BASETEMP_INIT) STATUS=0; \
	$(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) tests/integration/ tests/link/ -m integration -v --tb=short --timeout=20 || STATUS=$$?; \
	if [ "$$STATUS" -ne 0 ] && [ "$$STATUS" -ne 5 ]; then exit $$STATUS; fi

# Run specific integration test
test-integration-only: .installed
	@if [ -z "$(TEST)" ]; then \
		echo "Usage: make test-integration-only TEST=<test_file_or_pattern>"; \
		echo "Example: make test-integration-only TEST=test_api.py"; \
		exit 1; \
	fi
	@$(PYTEST_BASETEMP_INIT) TARGET="$(TEST)"; \
	case "$$TARGET" in \
		tests/*|-*) ;; \
		*) TARGET="tests/integration/$$TARGET" ;; \
	esac; \
	STATUS=0; \
	$(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) "$$TARGET" --timeout=20 || STATUS=$$?; \
	if [ "$$STATUS" -ne 0 ] && [ "$$STATUS" -ne 5 ]; then exit $$STATUS; fi

# Run all tests (core + apps + integration)
test-all: .installed
	@echo "Running all tests (core + apps + integration)..."
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) tests/ -v --cov=. --ignore=tests/integration $(NOT_INTEGRATION) && $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) solstone/apps/ -v --cov=. --cov-append

# Auto-format and fix code, then report any remaining issues
format: .installed
	@echo "Formatting and fixing code with ruff..."
	@$(RUFF) format .
	@$(RUFF) check --fix .
	@echo ""
	@echo "Checking for remaining issues..."
	@RUFF_OK=true; MYPY_OK=true; \
	$(RUFF) check . || RUFF_OK=false; \
	$(MYPY) . || MYPY_OK=false; \
	if $$RUFF_OK && $$MYPY_OK; then \
		echo ""; \
		echo "All clean!"; \
	else \
		echo ""; \
		echo "Issues above need manual fixes."; \
	fi

# Clean build artifacts and cache files
clean:
	@echo "Cleaning build artifacts and cache files..."
	rm -rf build/ dist/ *.egg-info/
	rm -rf .pytest_cache/ .coverage .mypy_cache/
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
	@echo "=== Checking extras consistency ==="
	@$(VENV_BIN)/python scripts/check_extras_consistency.py
	@echo ""
	@echo "=== Running mypy ==="
	@$(MYPY) . || true
	@echo ""

ci: install-checks
	@echo "=== Running tests ==="
	@$(MAKE) test-cov
	@echo ""
	@echo "All CI checks passed!"

verify: install-checks
	@echo "=== Running tests ==="
	@$(MAKE) test-cov
	@echo ""
	@echo "Verification complete!"

# Watch for changes and run tests (requires pytest-watch)
watch: .installed
	@$(UV) pip show pytest-watch >/dev/null 2>&1 || { echo "Installing pytest-watch..."; $(UV) pip install pytest-watch; }
	$(VENV_BIN)/ptw -- -q

# Generate coverage report (core + apps, excluding core integration tests)
coverage: .installed
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) tests/ --cov=. --cov-report=html --cov-report=term --ignore=tests/integration $(NOT_INTEGRATION)
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) solstone/apps/ --cov=. --cov-report=html --cov-report=term --cov-append
	@echo "Coverage report generated in htmlcov/index.html"

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
	@$(UV) pip list | grep -E "^(pytest|ruff|mypy|Flask|numpy|Pillow|openai|anthropic|google-genai)" || true

# Install pre-commit hooks (if using pre-commit)
pre-commit: .installed
	@$(UV) pip show pre-commit >/dev/null 2>&1 || { echo "Installing pre-commit..."; $(UV) pip install pre-commit; }
	$(VENV_BIN)/pre-commit install
	@echo "Pre-commit hooks installed!"

# Low-bar layer-hygiene check (see docs/coding-standards.md § Layer Hygiene)
check-layer-hygiene: .installed
	$(VENV_BIN)/python scripts/check_layer_hygiene.py

# Re-run the live four-backend integrated-façade cogitate smoke. Spawns the
# archived runner (extro `vpe/workspace/archived/`) against this venv so the
# real openhands-sdk Agent path is exercised end-to-end. Requires real API
# keys in env (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`) and
# `llama-server` on PATH for the `local` backend. Catches v1.23-style Agent
# schema regressions that the openhands-fake unit tests cannot — see
# `tests/integration/test_cogitate_facade_agent_construction.py` for the
# pytest variant that runs without keys.
COGITATE_SMOKE_RUNNER ?= /home/jer/projects/extro/vpe/workspace/archived/cogitate-integrated-facade-smoke-260523.py

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
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) \
	  tests/integration/test_bundled_install_real_uv.py \
	  tests/integration/test_bundled_provider_migration.py \
	  tests/integration/test_local_install_canonical.py \
	  -m integration -v --tb=short --timeout=120
	$(PYTEST_BASETEMP_INIT) $(TEST_ENV) $(PYTEST) $(PYTEST_BASETEMP_FLAG) \
	  solstone/apps/settings/tests/test_providers_panel_visual.py \
	  -m integration -v --tb=short --timeout=120

release: ## Publish solstone to PyPI (production)
	@bash scripts/release.sh

release-test: ## Publish solstone to TestPyPI
	@bash scripts/release.sh --test
