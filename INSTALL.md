# installing solstone

these instructions are for a coding agent and human working together. solstone is your co-brain — your observers experience your day along with you, sol curates your memories, and your journal holds everything. open source, made by sol pbc.

**supported platforms:** linux (primary), macOS. windows is not yet supported.

the latest version of these instructions is at https://solstone.app/install.

## before you begin

### check whether solstone is already installed

```bash
sol --version 2>&1 && sol service status 2>&1
```

if `sol` isn't on PATH, the install hasn't been done yet — proceed.
if solstone is running and healthy, skip to [install an observer](#install-an-observer).

### prerequisites

linux: install `uv` (`curl -LsSf https://astral.sh/uv/install.sh | sh`) and `ripgrep` (`rg`) from your distro package manager.

macOS: install xcode command line tools (`xcode-select --install`) and homebrew (https://brew.sh), then `brew install uv ripgrep`.

## install

```bash
uv tool install solstone
```

(or `pipx install solstone` if you prefer pipx — they're equivalent for our purposes.)

`uv tool install` puts `sol` at `~/.local/bin/sol`, which most shells already have on PATH. if not: `exec $SHELL -l` or restart your shell.

## set up

```bash
sol setup
```

this runs doctor diagnostics, confirms the journal directory at `~/journal`, installs the local transcription model (~2.5 GB on linux), installs the solstone skill for claude code, codex, and gemini, installs all journal-side talent skills into the configured journal so cogitate sub-agents can discover them, and starts a background service (systemd on linux, launchd on macOS) listening on http://localhost:5015.

let your human know: **open http://localhost:5015 in a browser**. the first-run wizard walks them through setting their identity and connecting a gemini API key. network access, and the password it requires, can be configured later in settings → security.

if a step has missing system libraries or python extras, `sol doctor` will tell you the exact install command to run for your platform. extras (`pdf`, `whisper`) can be added at any time with `uv tool upgrade solstone --extra pdf` or `pip install 'solstone[pdf]'`. on linux, local parakeet transcription needs `solstone[parakeet-onnx-cpu]` (or `[parakeet-onnx-cuda]` for NVIDIA GPUs); install or upgrade the same way as other extras.

if the service fails to start, check `sol service logs`.

## choosing how to power sol

the sol agent is powered by an AI model, and you choose which. the choice has real privacy and hardware trade-offs worth understanding before you invest time in a path.

- **a hosted provider key is the recommended way to start.** point solstone at Google (Gemini), OpenAI, or Anthropic with **your own developer API key**, created in that provider's developer console — *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). this is the fastest path to a working co-brain and what the first-run wizard sets up.
- **a local model via Ollama is a real, supported goal, but not the default daily experience yet.** running the sol agent fully locally means nothing leaves your machine. it's the maximum-privacy path, but it needs capable hardware and a local model with strong "thinking" support; smaller models on constrained machines (for example a base Mac mini) struggle on the reasoning-heavy work. treat local as a goal to grow into, not the recommended starting point.

a hardware heads-up: local transcription alone installs a ~2.5 GB model, and a capable local *thinking* model needs meaningfully more memory and compute on top of that. if your machine is constrained, start with a hosted key and revisit local later; you can switch any time in settings → providers.

what actually leaves your machine differs sharply between these paths: with a local model, nothing leaves; with a hosted provider, only that task's prompt plus the relevant journal context goes, directly to that provider under your own key. solstone is never a proxy, and sol pbc is never in that path and never sees it. for the full picture of what's sent, to whom, and under whose terms, see [what solstone sends](DATA-FLOW.md).

## install an observer

solstone needs a platform observer alongside your journal.

```bash
sol observer install                    # uses hostname as stream name
sol observer install laptop             # named stream
sol observer install laptop --platform linux
sol observer install --platform tmux
sol observer install --dry-run          # preview only
```

on macOS, `sol observer install --platform macos` directs you to the signed app bundle at https://solstone.app/observers.

## upgrading

```bash
uv tool upgrade solstone && sol setup
```

(or `pipx upgrade solstone && sol setup`.) the second command refreshes the runtime artifacts and reconciles the service unit if anything has changed.

## done

once the observer is running, your observers experience your day along with you, transcribe conversations, surface people and projects, build a knowledge graph, and make everything searchable at http://localhost:5015. everything stays in your journal — one folder per day.

source code: https://github.com/solpbc/solstone
company: https://solpbc.org

## feedback

questions, feedback, or a bug? **follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone/issues for bugs, or reach support at https://support.solstone.app. you don't need to know anyone — those are the front doors.

(running into trouble or want to develop on solstone yourself? see [CONTRIBUTING.md](CONTRIBUTING.md).)
