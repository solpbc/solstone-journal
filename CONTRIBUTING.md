# Contributing to solstone

Thank you for your interest in contributing to solstone. This guide covers developing on solstone from a source checkout. If you just want to run the software, see [INSTALL.md](INSTALL.md).

## Prerequisites

solstone development uses a source checkout, a repo-local Python environment, and the `uv` package manager.

Required everywhere:

- Python 3.12 or later, as declared in `pyproject.toml`
- [uv](https://docs.astral.sh/uv/)
- Git
- ripgrep (`rg`)
- ffmpeg for audio processing
- minisign 0.12 exactly; `scripts/transparency_signing.py` enforces this version

Linux is the primary development platform. macOS is supported. Source-checkout installs on Apple Silicon need Xcode command line tools to build the CoreML parakeet helper. Owner-facing installs are the relocatable tree in [INSTALL.md](INSTALL.md), not a pip/uv/pipx package.

Linux source builds additionally require Clang development headers. Linux/x86_64
also requires NASM; omit `nasm` from the commands below on Linux/aarch64.
⚠ Nothing enforces this architecture split before compilation any more; `make preflight` did, and it went with the Python reference cut. Check the requirements above by hand.
Install the exact minisign 0.12 binary from the
[upstream 0.12 release](https://github.com/jedisct1/minisign/releases/tag/0.12)
rather than relying on an unpinned distro package, then confirm `minisign -v`
prints `minisign 0.12`.

Fedora/RHEL:

```bash
sudo dnf install python3 git ripgrep ffmpeg nasm clang-devel libgomp pipewire gstreamer1-plugins-base gstreamer1-plugin-pipewire pulseaudio-utils
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Ubuntu/Debian:

```bash
sudo apt install python3 git ripgrep ffmpeg nasm libclang-dev libgomp1 pipewire gstreamer1.0-tools gstreamer1.0-pipewire pulseaudio-utils
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Arch:

```bash
sudo pacman -S python git ripgrep ffmpeg nasm clang libgomp pipewire gstreamer gst-plugin-pipewire libpulse
curl -LsSf https://astral.sh/uv/install.sh | sh
```

macOS:

```bash
xcode-select --install
brew install python git ripgrep ffmpeg uv
```

## Source-checkout install

```bash
git clone https://github.com/solpbc/solstone-journal.git
cd solstone-journal
make install
.venv/bin/journal setup
```

`make install` creates `.venv/`, syncs dependencies from `pyproject.toml` and `uv.lock`, installs the package in editable mode, regenerates router skill references, and refreshes the `solstone` + `journal` project skill symlinks into the journal.

In a source checkout, bare `uv sync` removes the published speakers-analyze helper because the workspace config prunes that package from the active dev environment; the `make speakers-analyze-helper` target that restored it was removed with the Python reference cut, so the helper must currently be reinstalled by hand. Use `uv sync --inexact` when you intentionally need a prune-free sync, which avoids the prune and is now the cleaner route. ⚠ If you do reinstall the helper by hand, note that from inside this workspace `uv pip install` can report success with exit code 0 while installing nothing unless `--no-config` is passed.

`.venv/bin/journal setup` runs doctor diagnostics, confirms the journal path, installs local transcription models, installs the `solstone` user skill for Claude Code / Codex / Gemini when those agents are configured, installs the `solstone` + `journal` router skills into the journal, creates or refreshes the source-checkout wrappers at `~/.local/bin/solstone` and `~/.local/bin/journal`, and starts the background service. The default web interface listens on http://localhost:5015. Use `.venv/bin/journal setup --port 8000` to choose another port on the first run.

After the first setup run, the wrapper lets you use `solstone` from anywhere:

```bash
journal service status
journal setup
```

The source-checkout journal lives at `journal/` inside the repo unless you pass `--journal` or have already configured another path.

Configure API keys in `journal/config/journal.json`. This file is the only key configuration method for source-checkout development:

```bash
mkdir -p journal/config
cat > journal/config/journal.json << 'EOF'
{
  "env": {
    "GOOGLE_API_KEY": "your-key-here"
  }
}
EOF
chmod 600 journal/config/journal.json
```

Replace `your-key-here` with your Google AI API key. Optional provider keys can be added to the same `env` object:

```json
{
  "env": {
    "GOOGLE_API_KEY": "your-gemini-key",
    "OPENAI_API_KEY": "your-openai-key",
    "ANTHROPIC_API_KEY": "your-anthropic-key"
  }
}
```

`journal.json` contains API keys and credentials. Keep it private and restricted (`chmod 600`).

### Seeding a dev/test journal from public media

If you want a journal seeded with public-domain audio and screen media instead of your own journal material — useful for contributors who shouldn't be exposed to a maintainer's personal journal, integration-test scenarios, or a clean dev environment — see [docs/FIELD_JOURNAL.md](docs/FIELD_JOURNAL.md). The `setup_field_journal.sh` script at the repo root populates `journal/chronicle/` from a local clone of [solpbc/field_journal](https://github.com/solpbc/field_journal). It is opt-in and deliberately not part of `make install` or `journal setup`.

## Repo layout

Start with [AGENTS.md](AGENTS.md) or [CLAUDE.md](CLAUDE.md) for the developer-facing repo map, layer hygiene rules, make targets, and coding invariants. Most implementation work lives in `core/crates/`, `core/payload/solstone/talent/`, and `core/native-sol/`.

For app work, read [docs/APPS.md](docs/APPS.md) before changing convey-shell or a `*-web` crate. For provider work, read [docs/PROVIDERS.md](docs/PROVIDERS.md). For journal layout, use `core/payload/solstone/talent/journal/SKILL.md`.

## Running the test suite

Use the Makefile targets. The high-signal commands are:

```bash
make test
make ci
make ci-full              # full operator final-tree gate
```

`make test` runs the selected Rust library/binary unit harnesses and prints its
source-derived omission boundary. The [Makefile](Makefile) is authoritative:
`make ci` is the efficient routine gate with formatting,
topology validation, library/binary Clippy, and
serialized library/binary unit tests. It does not run Cargo integration-test
targets or heavyweight native, platform, and policy legs. An operator runs the
selectable, registry-driven `make ci-full` gate on the exact final-tree SHA
after `make ci-full-prep`.

⚠ **There is no Python test suite.** It was removed with the Python reference tree, along with the `make test-*` targets that drove it. Run focused Rust tests directly. In the four behavior-classified packages, a default-feature `--lib`/`--bins` selection contains only routine same-crate evidence; run the matching classified full-test and full-Clippy Make targets for broader `full-tests` same-crate evidence. For example:

```bash
cargo test --manifest-path core/Cargo.toml -p solstone-core-facets --lib
make check-rust-classified-full-tests-facets
make check-rust-classified-full-clippy-facets
```

⚠ **`make verify-api` and `make verify-schemathesis` are also gone**, because each drove
a deleted Python file, so nothing checks API baselines automatically today. For
user-visible web changes, review the live UI in a sandbox:

```bash
make sandbox       # start a sandbox to review the live UI (make sandbox-stop when done)
```

See [AGENTS.md](AGENTS.md) for the full Makefile command table and [docs/testing.md](docs/testing.md) for test isolation details.

## Developing on AI features

### macOS Apple Silicon: CoreML-accelerated parakeet

Packaged installs of solstone on Apple Silicon Macs running macOS 14 or newer ship the CoreML transcription helper as a pre-built, signed, and notarized binary. No build step is required for owners using a packaged install.

Source-checkout installs build the helper locally so you can iterate on the Swift source:

```bash
make parakeet-helper
```

The built binary lives at:

```text
core/crates/solstone-core-transcribe/parakeet-helper/.build/release/parakeet-helper
```

If you change the helper source, rebuild it before testing the CoreML parakeet path. The runtime resolver prefers a `_bin/parakeet-helper` next to the package (a locally staged signed copy) over `.build/release/`. Installed trees look next to the running `solstone-core` binary, then `bin/parakeet-helper` and `lib/parakeet-helper/parakeet-helper`. Override with `SOLSTONE_PARAKEET_HELPER`.

### Skills and talents

Talent prompts live under `core/payload/solstone/talent/<name>.md`; apps may add app-specific talent files under `core/payload/solstone/apps/<app>/talent/`. Talent frontmatter declares type, schedule, provider/model behavior, hooks, priority, and output expectations.

The installed project skills are the two router skills under `core/payload/solstone/talent/solstone/` and `core/payload/solstone/talent/journal/`. App command fragments under `core/payload/solstone/apps/<app>/talent/<app>/SKILL.md` feed the generated router references; they are not installed as top-level skills.

After changing a router skill or an app command fragment, run:

```bash
make skills
```

That target first runs `scripts/build_skill_references.py` to regenerate the checked-in references, then refreshes the `solstone` + `journal` router skill symlinks inside the journal. `make install` also runs this target. Run `make check-skill-references` directly, or use `make install-checks`, to catch stale generated references.

## Migrating from a source install to a tree install

A tree install puts `solstone` and `journal` on PATH directly. See [INSTALL.md](INSTALL.md). It does not use the source-checkout managed wrapper, and it does not use `.venv/bin/solstone`.

`make uninstall` is disabled by design. To migrate cleanly from a source checkout to a tree install, remove user-runtime artifacts explicitly:

```bash
journal service uninstall
solstone skills uninstall
```

Then install the tree from [INSTALL.md](INSTALL.md) and run `journal setup`.

Your journal is preserved at `~/journal`; solstone does not remove it during install or uninstall. Do not add backwards-compatibility shims for the old source-checkout layout. This migration is a clean break.

## License of Contributions

By contributing to this repository, you agree that your contributions are
licensed under the GNU Affero General Public License v3.0 (AGPL-3.0-only),
the same license as the project.

You represent that you have the right to submit the contribution and that it
does not include proprietary, confidential, or third-party code that is
incompatible with the AGPL.

## Developer Certificate of Origin (DCO)

All contributions must be signed off using:

    git commit -s

This certifies compliance with the Developer Certificate of Origin (DCO).
