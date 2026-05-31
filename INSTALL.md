# installing solstone

these instructions are for a coding agent and human working together. solstone is your co-brain — your observers experience your day along with you, sol curates your memories, and your journal holds everything. open source, made by sol pbc.

**supported platforms:** linux (primary), macOS. windows is not yet supported.

the latest version of these instructions is at https://solstone.app/install.

## before you begin

### check whether solstone is already installed

```bash
sol --version 2>&1 && journal service status 2>&1
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
journal setup
```

this runs the setup readiness doctor battery, confirms the journal directory at `~/journal`, installs the local transcription model (~2.5 GB on linux), installs the solstone skill for claude code, codex, and gemini, installs all journal-side talent skills into the configured journal so cogitate sub-agents can discover them, and starts a background service (systemd on linux, launchd on macOS) listening on http://localhost:5015.

let your human know: **open http://localhost:5015 in a browser**. the first-run wizard walks them through setting their identity and connecting a gemini API key. network access, and the password it requires, can be configured later in settings → security.

if the readiness doctor step (`sol doctor --readiness`) finds missing system libraries or python extras, it will tell you the exact install command to run for your platform. extras (`pdf`, `whisper`) can be added at any time with `uv tool upgrade solstone --extra pdf` or `pip install 'solstone[pdf]'`. on linux, the default parakeet transcription works out of the box — its runtime ships with the install and `journal setup` downloads the model, so there's no extra to add. NVIDIA GPU owners who want GPU-accelerated transcription can add `solstone[parakeet-onnx-cuda]`; `sol doctor` reports whether the default backend's runtime and model are ready.

if the service fails to start, check `journal service logs`.

## choosing how to power sol

the sol agent is powered by an AI model, and you choose which. the choice has real privacy and hardware trade-offs worth understanding before you invest time in a path.

- **a hosted provider key is the recommended way to start.** point solstone at Google (Gemini), OpenAI, or Anthropic with **your own developer API key**, created in that provider's developer console — *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). this is the fastest path to a working co-brain and what the first-run wizard sets up. cogitate (sol's tool-calling agent loop, used by chat/digest/morning_briefing/etc.) works out of the box as soon as you set a provider key — no extra install step.
- **a local model via the local provider is a real, supported goal, but not the default daily experience yet.** running the sol agent fully locally means nothing leaves your machine. it's the maximum-privacy path, but it needs capable hardware and a local model with strong "thinking" support; smaller models on constrained machines (for example a base Mac mini) struggle on the reasoning-heavy work. treat local as a goal to grow into, not the recommended starting point.
- **on Apple Silicon, you can run sol's screen analysis on-device today.** macs with Apple Silicon and at least 16 GB of memory can turn on the local provider in settings → providers; journal downloads a local model once, then does the work of making sense of your screen entirely on your machine, with nothing sent to a cloud provider. it's opt-in and covers screen analysis for now; the rest of sol stays on whichever provider you chose above.

a hardware heads-up: local transcription alone installs a ~2.5 GB model, and a capable local *thinking* model needs meaningfully more memory and compute on top of that. if your machine is constrained, start with a hosted key and revisit local later; you can switch any time in settings → providers.

what actually leaves your machine differs sharply between these paths: with a local model, nothing leaves; with a hosted provider, only that task's prompt plus the relevant journal context goes, directly to that provider under your own key. solstone is never a proxy, and sol pbc is never in that path and never sees it. for the full picture of what's sent, to whom, and under whose terms, see [what solstone sends](DATA-FLOW.md).

## install an observer

solstone needs a platform observer alongside your journal. observers are independent packages — install one for each machine you want to observe along with you.

**macOS:** download the signed app bundle from https://solstone.app/observers and drag it to Applications. it pairs itself with the running journal on first launch.

**linux:**

```bash
pipx install solstone-linux
solstone-linux install-service
sol observer create laptop      # mint a key for this observer
```

`solstone-linux install-service` walks you through pointing the observer at the key you just minted. swap `laptop` for any name you'd like to identify this machine by.

**tmux terminal sessions:**

```bash
pipx install solstone-tmux
solstone-tmux install-service
sol observer create tmux-laptop
```

(use `uv tool install` in place of `pipx install` if you prefer uv — they're equivalent.)

## upgrading

```bash
uv tool upgrade solstone && journal setup
```

(or `pipx upgrade solstone && journal setup`.) the second command refreshes the runtime artifacts and reconciles the service unit if anything has changed.

## uninstall

1. remove setup-managed runtime files: `journal setup --clean-uninstall`
   this removes the user service, managed `~/.local/bin/sol` wrapper, user config, and setup manifest. it does not remove your journal.
2. optional: remove agentic-tooling skills: `sol skills uninstall`.
3. uninstall the python package: `uv tool uninstall solstone` (or `pipx uninstall solstone`).
4. macOS only: drag `/Applications/solstone.app` to Trash.
5. macOS only, optional: remove observer app data and the parakeet model cache:
   ```bash
   rm -rf ~/Library/Application\ Support/solstone/
   ```
   this evicts the ~2.5 GB parakeet cache; reinstall will re-download it.
6. macOS only, optional: reset privacy permissions:
   ```bash
   tccutil reset Microphone app.solstone.observer && tccutil reset ScreenCapture app.solstone.observer
   ```
   or use System Settings → Privacy & Security.

## done

once the observer is running, your observers experience your day along with you, transcribe conversations, surface people and projects, build a knowledge graph, and make everything searchable at http://localhost:5015. everything stays in your journal — one folder per day.

source code: https://github.com/solpbc/solstone-journal
company: https://solpbc.org

## feedback

questions, feedback, or a bug? **follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone-journal/issues for bugs, or reach support at https://support.solstone.app. you don't need to know anyone — those are the front doors.

(running into trouble or want to develop on solstone yourself? see [CONTRIBUTING.md](CONTRIBUTING.md).)
