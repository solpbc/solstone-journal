# installing solstone

these instructions are for a coding agent and human working together. solstone is the platform: sol is the app, and the journal is the memory it keeps. sol lives on your devices, experiences your day with you, and keeps it all in your journal — always private, only yours. open source, made by sol pbc.

**supported platforms:** linux (primary), macOS. windows is not yet supported.

the latest version of these instructions is at https://solstone.app/install.

## before you begin

### check whether solstone is already installed

```bash
sol --version 2>&1 && journal service status 2>&1
```

if `sol` isn't on PATH, the install hasn't been done yet — proceed.
if solstone is running and healthy, skip to [install sol on your devices](#install-sol-on-your-devices).

### prerequisites

linux: install `uv` (`curl -LsSf https://astral.sh/uv/install.sh | sh`),
`ripgrep` (`rg`), and the system OpenMP runtime used by the default local
Parakeet transcription provider:

```bash
sudo apt install libgomp1      # Ubuntu/Debian
sudo dnf install libgomp       # Fedora/RHEL
sudo pacman -S libgomp         # Arch
```

macOS: install xcode command line tools (`xcode-select --install`) and homebrew (https://brew.sh), then `brew install uv ripgrep`.

## install

most people install solstone to **run the journal here** — the full host that
transcribes, makes sense of your day, and holds everything:

```bash
pip install solstone-journal                                    # includes sol on PATH
uv tool install solstone-journal && uv tool install solstone   # journal tool + sol tool
pipx install solstone-journal && pipx install solstone         # same two-tool story
```

Pick one installer.

A pip install of solstone-journal puts `sol`, `solstone`, and `journal` on PATH.
uv tool and pipx expose each installed package's own commands: the journal
package provides `journal`, and the thin command package provides `sol` and `solstone`,
which is why their install lines above are two commands. The
[package metadata](packages/solstone-journal/pyproject.toml) and
[cleanroom installer checks](scripts/cleanroom-install.sh) pin this command
split.

NVIDIA GPU owners who want GPU-accelerated transcription install
`solstone-journal-cuda` **instead of** `solstone-journal` with the same
installer command shape.

### just the `sol` client

`pip install solstone` (no extras) installs only the thin `sol` access client —
talk to a journal running **elsewhere** (a second machine, or a journal you
reach over your private link). it carries none of the journal host's AI/media
stack, so it's small and fast:

```bash
uv tool install solstone        # the sol client, on PATH
uvx solstone --help             # or ephemerally — no install, one-shot
```

Running the journal? Check the machine first: `uvx solstone check` renders a one-shot readiness verdict (GPU, memory, free disk) for the bundled local models — no install required. It exits 0 when ready, and prints exactly what's missing when not.

A thin/no-extras install carries only `sol` and `solstone`. Install the journal
to use `journal setup` and `journal start`.

## set up

```bash
journal setup
```

this runs the setup readiness doctor battery, confirms the journal directory at `~/journal`, installs the local transcription model (~2.5 GB on linux), installs the `sol` skill for claude code, codex, and gemini, installs the journal-side `sol` and `journal` router skills so sol can tend the journal, and starts a background service (systemd on linux, launchd on macOS) listening on http://localhost:5015.

let your human know: **open http://localhost:5015 in a browser**. the first-run wizard walks them through setting their identity and choosing how sol thinks — local by default (the bundled model runs right in the journal), or their own provider key if the machine can't run a local model.

a `solstone-journal` install bundles the Python and native artifacts a journal
host needs — PDF rendering, whisper, and the default CPU transcription stack
are included; `journal setup` downloads the transcription model. on Linux, the
host supplies the small system OpenMP runtime listed in prerequisites.
`journal doctor --readiness` runs the actual Parakeet binary before reporting
it ready and gives the exact package-manager command when that runtime is
missing.

Pick one of `solstone-journal` or `solstone-journal-cuda` — the CPU and GPU ONNX runtimes share the same files and must not both be installed. `journal doctor` reports whether the transcription runtime, native speaker-analysis helper, and bundled models are ready.

This CUDA extra is only for transcription. The Linux local model provider picks its own GPU backend: on NVIDIA GPUs from the RTX 30 series up with a current NVIDIA driver (580 series or newer), it runs natively on CUDA; the runtime downloads from updates.solstone.app as a checksum-pinned bundle. On other hardware GPUs (AMD, Intel, or older NVIDIA) it uses Vulkan. CPU/software Vulkan devices are rejected instead of falling back silently. On AMD, the local model path runs through Mesa/RADV Vulkan, while transcription stays on the bundled CPU runtime.

if the service fails to start, check `journal service logs`.

## choosing how to power sol

sol is powered by an AI model, and it runs **locally by default** — on every device. the bundled model runs right in your journal, so your thinking never leaves the machine. a cloud lane is an option, not the default; you choose in settings → providers (or the thinking app).

- **local built-in — the default.** on a capable machine sol thinks locally with nothing extra to set up — the bundled model handles both the reasoning over your journal and screen analysis, and nothing is sent to a cloud provider. the practical floor is about **8 GB**: an 8 GB GPU, or 8 GB of free memory on Apple Silicon (the model is ~2.74 GB on disk, plus the ~2.5 GB transcription model). solstone checks first and won't activate a local model that won't fit; on Linux it needs a supported hardware GPU (NVIDIA via CUDA on RTX 30-series or newer, or AMD, NVIDIA, and Intel via Vulkan).
- **an engine you bring yourself, if your machine can't clear that bar — or you'd rather not spend its power.** point solstone at Google (Gemini), OpenAI, or Anthropic with **your own developer API key**, created in that provider's developer console — *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). you can also point it at your own endpoint instead of a cloud provider — a model you run yourself, on this machine or another one you control. you can switch any time in settings → providers.
- **confidential processing, if you'd rather not run it yourself or hand it to a provider.** sol's thinking runs on a sol pbc endpoint — verified by attestation before anything is sent, processed in memory, and not retained. it is off unless you turn it on, and only runs while it is enabled.

what actually leaves your machine differs sharply between these paths: with the local model, nothing leaves; with a hosted provider, only that task's prompt plus the relevant journal context goes, directly to that provider under your own key — on that path solstone is never a proxy, and sol pbc is never in it and never sees it. there is a third case, and it is the one exception: if you turn on confidential processing, sol's thinking runs on a sol pbc endpoint — only while you have it enabled, verified by attestation before anything is sent, processed in memory, and not retained. for the full picture of what's sent, to whom, and under whose terms, see [what solstone sends](DATA-FLOW.md).

## install sol on your devices

your journal needs sol alongside it — sol takes in your day on each device and keeps it in your journal. each platform ships its own package; install one for each machine you want sol on.

**macOS:** download the signed app bundle from https://solstone.app/observers and drag it to Applications. it pairs itself with the running journal on first launch.

**linux:**

```bash
pipx install solstone-linux
solstone-linux install-service
```

`solstone-linux install-service` walks you through pairing this machine with your running journal. choose any name you'd like to identify this machine by.

**tmux terminal sessions:**

```bash
pipx install solstone-tmux
solstone-tmux install-service
```

(for these packages, `uv tool install solstone-tmux` is also fine if you prefer uv.)

## migrating from a pre-split install

If you installed the journal before the package split, run the one-time migration
line for your installer family, then run `journal setup`.

```bash
pip uninstall solstone-journal-host && pip install solstone-journal
pipx uninstall solstone && pipx install solstone-journal && pipx install solstone
uv tool uninstall solstone && uv tool install solstone-journal && uv tool install solstone
```

## upgrading

```bash
pip install --upgrade solstone-journal && journal setup
uv tool install --upgrade solstone-journal && uv tool install --upgrade solstone && journal setup
pipx upgrade solstone-journal && journal setup
```

Use the same installer family you used for install. The `uv tool` form upgrades both
the journal package and the thin `solstone` client, while the pipx form upgrades
the journal tool. For GPU transcription, upgrade `solstone-journal-cuda` instead
of `solstone-journal`. The `journal setup` step refreshes runtime artifacts and
reconciles the systemd/launchd unit if anything has changed.

## upgrading an existing journal

The steps above cover the package upgrade. If you already have a journal with history in it, two more things happen (or need to happen) on top of that:

**search index rebuild, if you're coming from before 0.7.0.** A pre-0.7.0 search index is dropped and rebuilt on first open after upgrade. The rebuild usually queues itself automatically, but if the service was still starting up when that happened, it can miss the window and print a message asking you to run it yourself. If search feels empty (or noticeably thinner than your journal's actual history) right after upgrading from a pre-0.7.0 install, run:

```bash
journal indexer --rescan-full
```

This is a full historical rescan — it can take a while on a large journal.

**connections/edges backfill, if you're coming from before 0.8.3.** The relationship layer between entities (who's connected to whom, and how) is derived by a separate pass. `journal indexer --rescan` (and the automatic light rescan on service start) only covers today plus facets/imports/apps — it does **not** backfill edges over your existing history. To build connections for days you already have:

```bash
journal indexer --rebuild-edges
```

Run this once after upgrading to a version with the connections feature if your existing days show no connections.

**if your journal isn't at the default location.** `journal setup` expects `~/journal` unless told otherwise. If your journal lives somewhere else — including if you're on an early source install that relied on the now-retired source-tree fallback (running `journal setup` from inside a git checkout used to default the journal into `<repo>/journal`) — point setup at it explicitly so it reuses that journal instead of creating an empty one at the default path:

```bash
journal setup --journal /path/to/your/journal --accept-existing-journal
```

Skipping `--journal` here silently resolves to `~/journal` and starts fresh — a real risk on the source-tree-fallback case, since a fresh `~/journal` looks like a working install even though your actual history is untouched at the old path.

## uninstall

1. remove setup-managed runtime files: `journal setup --clean-uninstall`
   this removes the user service, managed `~/.local/bin/sol` wrapper, user config, and setup manifest. it does not remove your journal.
2. optional: remove the installed `sol` agent skill: `sol skills uninstall`.
3. uninstall the python packages: `uv tool uninstall solstone-journal && uv tool uninstall solstone` (or `pipx uninstall solstone-journal`).
4. macOS only: drag `/Applications/solstone.app` to Trash.
5. macOS only, optional: remove sol's app data and the parakeet model cache:
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

once it's running, sol experiences your day along with you and keeps it all in your journal — conversations transcribed, people and projects surfaced, a knowledge graph built, everything searchable at http://localhost:5015. your journal is one folder per day, always private, only yours.

source code: https://github.com/solpbc/solstone-journal
company: https://solpbc.org

## feedback

questions, feedback, or a bug? **follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone-journal/issues for bugs, or reach support at https://support.solstone.app. you don't need to know anyone — those are the front doors.

(running into trouble or want to develop on solstone yourself? see [CONTRIBUTING.md](CONTRIBUTING.md).)
