# Contributing to solstone

Thank you for your interest in contributing to solstone. This guide covers developing on solstone from a source checkout. If you just want to run the software, see [INSTALL.md](INSTALL.md).

## Prerequisites

solstone development uses a source checkout, a repo-local Python environment, and the `uv` package manager.

Required everywhere:

- Python 3.11 or later
- [uv](https://docs.astral.sh/uv/)
- Git
- ripgrep (`rg`)
- ffmpeg for audio processing

Linux is the primary development platform. macOS is supported. Source-checkout installs on Apple Silicon need Xcode command line tools to build the CoreML parakeet helper; packaged installs (`uv tool install solstone`) on macOS 14 or newer ship the helper as a pre-built binary.

Fedora/RHEL:

```bash
sudo dnf install python3 git ripgrep ffmpeg pipewire gstreamer1-plugins-base gstreamer1-plugin-pipewire pulseaudio-utils
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Ubuntu/Debian:

```bash
sudo apt install python3 git ripgrep ffmpeg pipewire gstreamer1.0-tools gstreamer1.0-pipewire pulseaudio-utils
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Arch:

```bash
sudo pacman -S python git ripgrep ffmpeg pipewire gstreamer gst-plugin-pipewire libpulse
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

`make install` creates `.venv/`, syncs dependencies from `pyproject.toml` and `uv.lock`, installs the package in editable mode, and refreshes the project skill symlinks into the journal.

`.venv/bin/journal setup` runs doctor diagnostics, confirms the journal path, installs local transcription models, installs the `solstone` skill for Claude Code when Claude is configured, creates or refreshes the source-checkout wrappers at `~/.local/bin/sol` and `~/.local/bin/journal`, and starts the background service. The default web interface listens on http://localhost:5015. Use `.venv/bin/journal setup --port 8000` to choose another port on the first run.

After the first setup run, the wrapper lets you use `sol` from anywhere:

```bash
journal service status
journal setup
```

The source-checkout journal lives at `journal/` inside the repo unless you pass `--journal` or have already configured another path.

Configure API keys and the web password in `journal/config/journal.json`. This file is the only key configuration method for source-checkout development:

```bash
mkdir -p journal/config
cat > journal/config/journal.json << 'EOF'
{
  "convey": {},
  "env": {
    "GOOGLE_API_KEY": "your-key-here"
  }
}
EOF
chmod 600 journal/config/journal.json
```

Run `journal password set` to configure web authentication. Replace `your-key-here` with your Google AI API key. Optional provider keys can be added to the same `env` object:

```json
{
  "convey": {},
  "env": {
    "GOOGLE_API_KEY": "your-gemini-key",
    "OPENAI_API_KEY": "your-openai-key",
    "ANTHROPIC_API_KEY": "your-anthropic-key"
  }
}
```

`journal.json` contains API keys and credentials. Keep it private and restricted (`chmod 600`).

### Seeding a dev/test journal from public media

If you want a journal seeded with public-domain audio and screen recordings instead of your own capture data — useful for contributors who shouldn't be exposed to a maintainer's personal journal, integration-test scenarios, or a clean dev environment — see [docs/FIELD_JOURNAL.md](docs/FIELD_JOURNAL.md). The `setup_field_journal.sh` script at the repo root populates `journal/chronicle/` from a local clone of [solpbc/field_journal](https://github.com/solpbc/field_journal). It is opt-in and deliberately not part of `make install` or `journal setup`.

## Repo layout

Start with [AGENTS.md](AGENTS.md) or [CLAUDE.md](CLAUDE.md) for the developer-facing repo map, layer hygiene rules, make targets, and coding invariants. Most implementation work lives in `solstone/think/`, `solstone/observe/`, `solstone/convey/`, `solstone/apps/`, `solstone/talent/`, and `tests/`.

For app work, read [docs/APPS.md](docs/APPS.md) before changing `solstone/apps/`. For provider work, read [docs/PROVIDERS.md](docs/PROVIDERS.md). For journal layout, use `solstone/talent/journal/SKILL.md`.

## Running the test suite

Use the Makefile targets. The high-signal commands are:

```bash
make test
make test-only TEST=tests/test_utils.py::test_foo
make test-apps
make test-app APP=settings
make ci
```

`make test` runs unit tests after a format check. `make ci` is the pre-commit gate: format-check, ruff, layer hygiene, and tests. Run it before committing.

Integration tests hit real provider APIs and require `.env` keys:

```bash
make test-integration
make test-integration-only TEST=tests/integration/test_foo.py
```

For user-visible web changes, use the sandbox/browser verification targets when relevant:

```bash
make verify-api
make verify-browser
make review
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
solstone/observe/transcribe/parakeet_helper/.build/release/parakeet-helper
```

If you change the helper source, rebuild it before testing the CoreML parakeet path. Note that the runtime resolver prefers `solstone/observe/transcribe/parakeet_helper/_bin/parakeet-helper` (the location populated by `make wheel-macos` for platform-wheel packaging) over the `.build/release/` path; if you previously ran `make wheel-macos`, run `make wheel-macos-clean` to clear the `_bin/` copy so your local rebuild takes effect.

### Skills and talents

Talent prompts live under `solstone/talent/<name>.md`; apps may add app-specific talent files under `solstone/apps/<app>/talent/`. Talent frontmatter declares type, schedule, provider/model behavior, hooks, priority, and output expectations.

Skills are `SKILL.md` files under `solstone/talent/` or `solstone/apps/*/talent/`. After adding or renaming a skill, run:

```bash
make skills
```

That refreshes the `.claude/` and `.agents/` skill symlinks inside the journal. `make install` also runs this target.

## Migrating from a source install to a packaged install

The packaged install (`uv tool install solstone`) installs `sol` to `~/.local/bin/sol` directly. It does not use the source-checkout managed wrapper, and it does not use `.venv/bin/sol`.

`make uninstall` is disabled by design. To migrate cleanly from a source checkout to a packaged install, remove user-runtime artifacts explicitly:

```bash
journal service uninstall
sol skills uninstall
python -m solstone.think.install_guard uninstall
uv tool install solstone
journal setup
```

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
