# installing solstone

these instructions are for a coding agent and human working together. solstone is the platform: sol is the app, and the journal is the memory it keeps. sol lives on your devices, experiences your day with you, and keeps it all in your journal — always private, only yours. open source, made by sol pbc.

**supported platforms:** linux, and macos on apple silicon. windows is not yet supported. the sol app already runs on mac; this guide is how you install the journal there too.

the latest version of these instructions is at https://solstone.app/install.

## before you begin

### check whether solstone is already installed

```bash
sol --version 2>&1 && journal service status 2>&1
```

if `sol` isn't on PATH, the install hasn't been done yet — proceed.
if solstone is running and healthy, skip to [install sol on your devices](#install-sol-on-your-devices).

### prerequisites

the journal ships as one self-contained tree. it needs no interpreter and no package manager of its own.

on linux, the system OpenMP runtime, used by the default local Parakeet transcription provider:

```bash
sudo apt install libgomp1      # Ubuntu/Debian
sudo dnf install libgomp       # Fedora/RHEL
sudo pacman -S libgomp         # Arch
```

⚠ the `.deb` and `.rpm` do not declare this one for you, and transcription is the part that stops working without it. `journal doctor` names it if it is missing.

## install the journal on linux

⚠ **linux only.** the tree is built for `linux-x86_64` and `linux-aarch64`, and the bootstrap refuses any other system. for mac, see [install the journal on mac](#install-the-journal-on-mac).

### where the files come from

the release channel is `updates.solstone.app`. `install.sh` accepts only that host, re-checking on every redirect hop; loopback is allowed for testing, and `--origin` overrides. `install.sh` lives in this repository at `core/distribution/install.sh`, and is served from the origin as well.

one command does the whole thing:

```bash
sh install.sh
```

that follows the `release` lane's `latest` pointer, then fetches the archive, its checksum and its release record from `updates.solstone.app`, verifies the digest, and installs. pass `--version <version>` to pin a version instead of following `latest`. pass `--lane staging` or `--lane dev` to install from a non-release lane (testers only; the default lane is `release`). the archive route below is the same operation with the files already on disk; the package route is your distribution's installer and does not verify a checksum for you.

every release names its files the same way: `solstone-journal-<version>-linux-<arch>`, where `<arch>` is `x86_64` or `aarch64`. the three archives are `.tar.gz`, `.deb` and `.rpm`; each release also carries a `.sha256`, a `.manifest.json` and a `.release` record.

### a distribution package

on Debian or Ubuntu:

```bash
sudo apt install ./solstone-journal-<version>-linux-x86_64.deb
```

on Fedora or RHEL:

```bash
sudo dnf install ./solstone-journal-<version>-linux-x86_64.rpm
```

either one puts `sol`, `solstone` and `journal` on PATH for every account on the machine.

### the archive

for a machine you do not administer, or a prefix you choose:

```bash
sh install.sh --archive solstone-journal-<version>-linux-x86_64.tar.gz \
              --sha256 solstone-journal-<version>-linux-x86_64.sha256 \
              --release solstone-journal-<version>-linux-x86_64.release
```

with no `--prefix` it installs under `~/.local/solstone-journal`, keeps each version in its own directory, and points a `current` symlink at the live one. it adds `current/bin` to PATH by writing a block into `~/.profile` between `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`. `--no-path` skips that edit, so a throwaway or side-by-side prefix does not touch your login files. on success it prints the version, the prefix, and how to pick up PATH.

⚠ **`~/.profile` is read by login shells.** a new terminal window on most linux desktops is not one, and zsh does not read it at all. either log out and back in, or:

```bash
. ~/.profile
```

**to verify the archive by hand** — `install.sh` already does this for you:

```bash
sha256sum --ignore-missing -c solstone-journal-<version>-linux-x86_64.sha256
```

⚠ `--ignore-missing` is not optional: the checksum file carries one line for each of the three archives, so a plain `-c` fails on the two you did not download.

### one tree, whichever machine

there is no separate download for talking to a journal running elsewhere. the tree carries `sol` and `solstone` alongside the journal binaries, so one install covers both roles. you carry a few binaries you will not run, and nothing else changes.

## install the journal on mac

apple silicon only. `install.sh` refuses any other mac by name.

⚠ **the tree is not published yet.** same origin and same bootstrap as linux, above. until the first release lands on `updates.solstone.app`, start from the files you have. `install.sh` lives in this repository at `core/distribution/install.sh`.

every release names its files `solstone-journal-<version>-macos-arm64`. the two containers are a `.tar.gz` and a signed, notarized, stapled `.pkg`. each release also carries a `.sha256`, a `.manifest.json`, a `.release` record, and a `.signing.json`.

### the archive

this is the route to run. it does not need administrator rights and it does not write `/usr/local`:

```bash
sh core/distribution/install.sh \
  --archive solstone-journal-<version>-macos-arm64.tar.gz \
  --sha256 solstone-journal-<version>-macos-arm64.sha256 \
  --release solstone-journal-<version>-macos-arm64.release
```

with no `--prefix` it installs under `~/.local/solstone-journal` and points a `current` symlink at the live version. on mac it writes the PATH block to both `~/.zprofile` (zsh, the login shell) and `~/.profile`. `--no-path` skips that edit, so a throwaway or side-by-side prefix does not touch your login files. on success it prints the version, the prefix, and how to pick up PATH.

macos logs you into zsh, which never reads `~/.profile`. open a new terminal, or:

```bash
. ~/.zprofile
journal --version
```

**to verify the archive by hand** — `install.sh` already does this for you. macos has no `sha256sum`; use:

```bash
shasum -a 256 -c solstone-journal-<version>-macos-arm64.sha256
```

the checksum file carries one line for each container. if you only have the tarball, the extra `.pkg` line will complain; that is the sidecar, not a failed digest.

### the package

the `.pkg` is the `/usr/local` route: same tree, signed with Developer ID Installer, notarized, and stapled. `/usr/local/bin` is already on the default PATH via `/etc/paths`.

```bash
sudo installer -pkg solstone-journal-<version>-macos-arm64.pkg -target /
```

that writes the live system prefix. do not run it on a machine whose `/usr/local` you are not ready to change.

sol on your mac still installs from its own signed bundle, under [install sol on your devices](#install-sol-on-your-devices). that is a different package from the journal.

## set up

```bash
journal setup
```

this runs the setup readiness doctor battery and confirms the journal directory at `~/journal`. it fetches the local transcription model (~1 GB), installs the `sol` skill for Claude Code, Codex, and Gemini, and installs the journal-side `sol` and `journal` router skills so sol can tend the journal. it then starts a background service (`systemd` on linux, `launchd` on mac at `~/Library/LaunchAgents/org.solpbc.solstone.plist`) listening on http://localhost:5015. the default port is shared across logins. a second journal on that port, including one started under another login, cannot bind it.

let your human know: **open http://localhost:5015 in a browser**. the first-run wizard walks them through setting their identity and choosing how sol thinks: local by default (the local model runs right on the machine), or their own provider key if the machine can't run one.

⚠ **the tree carries the binaries the journal needs to run, not the transcription stack.** the Parakeet transcription helper and its model are fetched during setup, by `journal install-models`. `journal doctor --readiness` runs the actual binary before reporting it ready, and on linux it gives the exact package-manager command when the system OpenMP runtime listed in prerequisites is missing.

`journal doctor` reports whether the transcription runtime, the native speaker-analysis helper, and the models they need are ready.

the linux local model provider picks its own GPU backend. on RTX 30, 40 and 50 series NVIDIA GPUs with a CUDA 13 driver it runs natively on CUDA, and the runtime downloads from `updates.solstone.app` as a checksum-pinned artifact. every other hardware GPU uses Vulkan. CPU and software Vulkan devices are rejected rather than falling back silently. transcription runs on the CPU runtime when the GPU cannot hold both it and the model.

if the service fails to start, check `journal service logs`.

## choosing how to power sol

sol is powered by an AI model, and it runs **locally by default**, on every device. the local model runs right on the machine your journal lives on, so nothing is sent to a cloud provider. a cloud option is available, not the default; you choose in settings → providers.

- **local built-in, the default.** on a capable machine sol thinks locally with nothing extra to set up. the local model handles both the reasoning over your journal and screen analysis. the floor is **6 GB of GPU memory** on linux, or a **16 GB Apple Silicon mac** (the model is ~3.4 GB on disk, plus the ~1 GB transcription model). solstone checks first and tells you what won't fit; on linux it also needs a supported hardware GPU (see [set up](#set-up)).
- **an engine you bring yourself**, if your machine can't clear that bar, or you'd rather not spend its power. point solstone at Google (Gemini), OpenAI, or Anthropic with **your own developer API key**, created in that provider's developer console, *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). you can also point it at your own endpoint instead of a cloud provider — a model you run yourself, on this machine or another one you control. you can switch any time in settings → providers.
- **confidential processing**, if you'd rather not run it yourself or hand it to a provider. available to approved scouts. let sol think off your device on confidential hardware sol pbc runs that keeps nothing. your journal must verify the service before anything is sent; if it can't verify, it doesn't send. while it's on, sol can transcribe your audio there too — transcribed, never kept; turn that off any time and transcription returns to your device.

what actually leaves your machine differs sharply between these paths. with the local model, nothing leaves. with a hosted provider, only that task's prompt plus the relevant journal context goes, directly to that provider under your own key; on that path solstone is never a proxy, and sol pbc is never in it. the third case is the one where sol pbc is in the path. with confidential processing on, sol's thinking is processed off your device, and with the audio setting on (it is on by default) speech-to-text runs there too — only while confidential processing is on, only over a channel your journal verified first, processed in memory and not retained. turn the audio setting off and transcription returns to your device. for the full picture of what's sent, to whom, and under whose terms, see [what solstone sends](DATA-FLOW.md).

## install sol on your devices

your journal needs sol alongside it: sol experiences your day with you on each device and keeps it in your journal. each platform ships its own package; install one for each machine you want sol on.

⚠ each of these has its own install guide and they are the current source of truth. the pip and pipx routes they used to document are legacy builds that no longer receive releases.

**mac:** download the signed app bundle from https://solstone.app/download and drag it to Applications. on first launch it finds the journal running on this machine; click **connect your journal** to pair.

**linux:** follow `solstone-linux`'s own INSTALL guide. installing the service and pairing it are two separate steps — `install-service` writes and starts the unit, and pairing reads a pair link you create in the journal.

**tmux terminal sessions:** follow `solstone-tmux`'s own INSTALL guide, which also carries the steps for retiring a previous Python installation.

## moving from a pip, uv or pipx install

**your journal itself is untouched by any of this.** it is a folder of dated directories and no installer owns it.

earlier releases installed the journal as a set of Python packages. the tree replaces them, and the two must not both be on PATH.

1. stop the service: `journal service stop`
2. remove the old packages with whichever installer put them there:
   ```bash
   pip uninstall -y solstone-journal solstone-journal-cuda solstone-journal-host solstone
   uv tool uninstall solstone-journal; uv tool uninstall solstone
   pipx uninstall solstone-journal; pipx uninstall solstone
   ```
3. install the tree as above, then run `journal setup`.

⚠ **there is no CUDA build of the tree.** if you were on `solstone-journal-cuda`, transcription moves to the CPU runtime — the same model, on the CPU, so long recordings take longer to process; nothing else about them changes. the local *model* provider still uses your GPU where it can. that path is separate and is described under [set up](#set-up).

## upgrading

install the newer package or archive the same way you installed the first one, then run `journal setup`. the setup step refreshes runtime artifacts and reconciles the service unit if anything has changed.

the archive route keeps each version in its own directory and moves the `current` symlink, so an upgrade unpacks a second tree alongside the live one before switching.

### if you already have a journal with history in it

a few more things happen, or need to happen, on top of the install.

**search index rebuild, on an older index schema.** an index on the older schema is dropped and rebuilt on first open after upgrade. the rebuild usually queues itself automatically, but if the service was still starting up when that happened, it can miss the window and print a message asking you to run it yourself. if search feels empty, or noticeably thinner than your journal's actual history, right after upgrading, run:

```bash
journal indexer --rescan-full
```

this is a full historical rescan and can take a while on a large journal.

**connections/edges backfill.** the relationship layer between entities (who's connected to whom, and how) is derived by a separate pass, and extraction is incremental on file modification time — so once a day's mtimes are recorded, an ordinary rescan will not re-extract its edges even with `--rescan-full`. a weekly schedule rebuilds them on its own. to force it now:

```bash
journal indexer --rebuild-edges
```

run this if your existing days show no connections and you would rather not wait for the weekly pass.

**if your journal isn't at the default location.** `journal setup` expects `~/journal` unless told otherwise. if your journal lives somewhere else, point setup at it explicitly so it reuses that journal instead of creating an empty one at the default path:

```bash
journal setup --journal /path/to/your/journal --accept-existing-journal
```

with no `--journal`, setup takes `SOLSTONE_JOURNAL`, then the `journal` key in `~/.config/solstone/config.toml`, then `~/journal`. on a machine with no prior config that last step starts fresh, and a fresh `~/journal` looks like a working install even though your actual history is untouched at the old path.

## uninstall

**none of this removes your journal.** it is a folder of dated directories and it survives every step below.

1. remove setup-managed runtime files: `journal setup --clean-uninstall --yes`
   this removes the service unit, the managed `sol` and `journal` wrappers in `~/.local/bin`, its config, and the setup manifest. without `--yes` it asks first; in a non-interactive shell that form refuses and exits 2, and nothing is removed. if the service cannot be removed, uninstall stops there and leaves the wrappers in place so you still have `journal` to retry.
2. optional: remove the installed `sol` agent skill: `sol skills uninstall`.
3. remove the tree, by the route you installed it:
   - `sudo apt remove solstone-journal` or `sudo dnf remove solstone-journal`
   - archive install: delete the prefix directory (`~/.local/solstone-journal` by default) and the PATH block `install.sh` added to `~/.profile` (and on mac, `~/.zprofile`), marked with `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`.
   - mac `.pkg` install: use the receipt the installer registered — it names every file the
     package put on disk.

     ⚠ **do not pipe `pkgutil --files` into `rm -rf`.** it lists directories too (`bin`, `lib`,
     `share`), and `/usr/local` is shared with Homebrew, Docker, VS Code and anything else on this
     machine — a raw `rm -rf` over the full list takes all of it with the package.

     ```bash
     pkgutil --files app.solstone.journal --only-files | while IFS= read -r f; do
       sudo rm -- "/usr/local/$f"
     done
     pkgutil --files app.solstone.journal --only-dirs \
       | awk '{ print gsub(/\//,"/"), $0 }' | sort -rn | cut -d' ' -f2- \
       | while IFS= read -r d; do
         sudo rmdir "/usr/local/$d" 2>/dev/null || true
       done
     sudo pkgutil --forget app.solstone.journal
     ```

     the first loop removes only the files the package installed — nothing else in `/usr/local` is
     touched. the second removes the directories it created, deepest first, and only the ones that
     are empty afterward; `rmdir` refuses a directory that still holds something else, so `bin`
     itself (shared with other tools) is left standing. the third clears the receipt.
4. mac only: drag `/Applications/solstone.app` to Trash.
5. mac only, optional: remove sol's app data and the Parakeet model cache:
   ```bash
   rm -rf ~/Library/Application\ Support/solstone/
   ```
   this evicts the Parakeet cache; reinstall will re-download it.
6. mac only, optional: reset privacy permissions:
   ```bash
   tccutil reset Microphone app.solstone.observer
   tccutil reset ScreenCapture app.solstone.observer
   ```
   or use System Settings → Privacy & Security.

## done

once it's running, sol experiences your day along with you and keeps it all in your journal — conversations transcribed, people and projects surfaced, a knowledge graph built, everything searchable at http://localhost:5015. your journal is one folder per day, always private, only yours.

source code: https://github.com/solpbc/solstone-journal
company: https://solpbc.org

## feedback

questions, feedback, or a bug? **follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone-journal/issues for bugs, or reach support at https://support.solstone.app. you don't need to know anyone — those are the front doors.

(running into trouble or want to develop on solstone yourself? see [CONTRIBUTING.md](CONTRIBUTING.md).)
