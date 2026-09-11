#!/usr/bin/env bash
# Read-only host readiness check for building solstone-journal from a source
# checkout. Checks presence only -- it never installs, downloads, or modifies
# anything. Run before `make build`/`make dev` on a fresh machine.
#
# Package-manager install commands are intentionally not duplicated here --
# see CONTRIBUTING.md's Prerequisites section for the exact per-OS command.
# This script's own fix text is reserved for the two checks that are not a
# simple "install this package" answer: the clang builtin-include path
# bindgen needs, and the exact minisign version this project pins.
set -u

cd "$(cd "$(dirname "$0")/.." && pwd)"

blocking_failed=0
warned=0

pass() { printf '  ok    %s\n' "$1"; }
fail() {
	printf '  FAIL  %s\n' "$1"
	[ -n "${2:-}" ] && printf '        -> %s\n' "$2"
	blocking_failed=1
}
warn() {
	printf '  warn  %s\n' "$1"
	[ -n "${2:-}" ] && printf '        -> %s\n' "$2"
	warned=1
}

os="$(uname -s)"
arch="$(uname -m)"

echo "solstone-journal build-environment preflight ($os/$arch)"
echo

# --- Rust toolchain ---
if command -v cargo >/dev/null 2>&1; then
	pass "cargo on PATH"
else
	fail "cargo on PATH" "install Rust via https://rustup.rs, then re-run"
fi

if command -v rustup >/dev/null 2>&1; then
	pass "rustup on PATH"
	channel="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml 2>/dev/null | head -1)"
	if [ -n "$channel" ]; then
		if rustup toolchain list 2>/dev/null | grep -qF "$channel"; then
			pass "pinned toolchain $channel installed"
		else
			warn "pinned toolchain $channel not yet installed" "cargo installs it automatically from rust-toolchain.toml on first build; run 'rustup toolchain install $channel' to pre-fetch it"
		fi
	fi
else
	fail "rustup on PATH" "install Rust via https://rustup.rs, then re-run"
fi

# --- clang builtin include path (bindgen / ffmpeg-sys-next) ---
if [ "$os" = "Linux" ]; then
	clang_include="$(ls -d /usr/lib/clang/*/include /usr/lib64/clang/*/include 2>/dev/null | head -1)"
	if [ -n "$clang_include" ]; then
		pass "clang builtin include headers found ($clang_include)"
	else
		fail "clang builtin include headers not found" \
			"install your distro's clang headers package (see CONTRIBUTING.md); if already installed, this is the 'limits.h' trap documented in AGENTS.md -- 'make'/'make ci'/'make test' compute the path automatically, but a bare 'cargo build' needs: export BINDGEN_EXTRA_CLANG_ARGS=-I\$(find /usr/lib{,64}/clang/*/include -print -quit 2>/dev/null)"
	fi
elif [ "$os" = "Darwin" ]; then
	if xcode-select -p >/dev/null 2>&1; then
		pass "Xcode command line tools installed"
	else
		fail "Xcode command line tools not installed" "run: xcode-select --install"
	fi
fi

# --- required binaries ---
for bin in ffmpeg rg; do
	if command -v "$bin" >/dev/null 2>&1; then
		pass "$bin on PATH"
	else
		fail "$bin on PATH" "install via your package manager -- see CONTRIBUTING.md Prerequisites for the exact command"
	fi
done

if [ "$os" = "Linux" ] && [ "$arch" = "x86_64" ]; then
	if command -v nasm >/dev/null 2>&1; then
		pass "nasm on PATH"
	else
		fail "nasm on PATH" "install via your package manager -- see CONTRIBUTING.md Prerequisites (linux/x86_64 only; not needed on aarch64)"
	fi
fi

# --- minisign, exact pinned version ---
if command -v minisign >/dev/null 2>&1; then
	version_line="$(minisign -v 2>&1 | head -1)"
	case "$version_line" in
	*0.12*) pass "minisign present ($version_line)" ;;
	*) fail "minisign wrong version ($version_line)" "install the exact 0.12 binary from https://github.com/jedisct1/minisign/releases/tag/0.12 -- distro packages often carry a different version" ;;
	esac
else
	fail "minisign on PATH" "install the exact 0.12 binary from https://github.com/jedisct1/minisign/releases/tag/0.12"
fi

# --- capture-path libs (advisory: affect audio/screen capture, not compiling) ---
if [ "$os" = "Linux" ]; then
	if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists gstreamer-1.0 2>/dev/null && pkg-config --exists libpipewire-0.3 2>/dev/null; then
		pass "gstreamer/pipewire dev libs found"
	else
		warn "gstreamer/pipewire dev libs not confirmed" "only needed for local audio/screen capture, not for compiling -- see CONTRIBUTING.md Prerequisites"
	fi
fi

echo
if [ "$blocking_failed" -ne 0 ]; then
	echo "preflight: one or more blocking checks failed; fix the items above before 'make build'/'make dev'."
	exit 1
elif [ "$warned" -ne 0 ]; then
	echo "preflight: all blocking checks passed; see warnings above."
	exit 0
else
	echo "preflight: all checks passed."
	exit 0
fi
