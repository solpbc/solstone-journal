# Installing solstone

These instructions are for a coding agent and human working together. solstone is a personal memory platform: the solstone app takes in what you share with it, and all of it goes into your journal. Your journal lives on a device you own; see [what material reaches your AI provider](DATA-FLOW.md) for material that leaves it. Open source, made by sol pbc.

**supported platforms:** linux, and macos on Apple Silicon. windows is not yet supported. The solstone app already runs on mac; this guide is how you install the journal there too.

The latest version of these instructions is at https://solstone.app/install.

## Before you begin

### Check whether solstone is already installed

```bash
solstone --version 2>&1 && journal service status 2>&1
```

If `solstone` is not on PATH, the install has not been done yet. Proceed.
If the solstone app is running and healthy, skip to [install the solstone app on your devices](#install-the-solstone-app-on-your-devices).

### Prerequisites

The journal ships as one self-contained tree. It needs no interpreter and no package manager of its own.

On linux, the system OpenMP runtime, used by the default local Parakeet transcription provider:

```bash
sudo apt install libgomp1      # Ubuntu/Debian
sudo dnf install libgomp       # Fedora/RHEL
sudo pacman -S libgomp         # Arch
```

⚠ The `.deb` and `.rpm` do not declare this one for you, and transcription is the part that stops working without it. `journal doctor` names it if it is missing.

## Install the journal on linux

⚠ **linux only.** the tree is built for `linux-x86_64` and `linux-aarch64`, and the bootstrap refuses any other system. For mac, see [install the journal on mac](#install-the-journal-on-mac).

### Where the files come from

The release channel is `updates.solstone.app`. `install.sh` accepts only that host, re-checking on every redirect hop; loopback is allowed for testing, and `--origin` overrides. `install.sh` lives in this repository at `core/distribution/install.sh`, and is served from the origin as well.

One command does the whole thing, once a release has been published:

```bash
sh install.sh
```

That follows the `release` lane's `latest` pointer, then fetches the archive, its checksum, and its release record from `updates.solstone.app`, verifies the digest, and installs. Pass `--version <version>` to pin a version instead of following `latest`. The archive route below is the same operation with the files already on disk.

Every release names its files the same way: `solstone-journal-<version>-linux-<arch>`, where `<arch>` is `x86_64` or `aarch64`. The three archives are `.tar.gz`, `.deb` and `.rpm`. Each release also carries a `.sha256`, a `.manifest.json`, a `.manifest.json.minisig` and a `.release` record.

### Verify first

One minisign signature covers every archive in the set. You run this. `apt` and `dnf` do not.

```bash
minisign -Vm solstone-journal-<version>-linux-x86_64.manifest.json \
  -p solstone-journal-release.pub
```

If it refuses, stop. Then install.

The public key is in this repository at `packaging/keys/solstone-journal-release.pub`. Once the channel is live it is also at `https://updates.solstone.app/solstone-journal/minisign.pub`. Install minisign from your distribution if you do not have it (`apt install minisign` or `dnf install minisign`).

`install.sh` checks the sha256. It does not check this signature. That is why this command comes first.

### The archive

This is the route that verifies a digest for you, after the signature check above. For a machine you do not administer, or a prefix you choose:

```bash
minisign -Vm solstone-journal-<version>-linux-x86_64.manifest.json \
  -p solstone-journal-release.pub
sh install.sh --archive solstone-journal-<version>-linux-x86_64.tar.gz \
              --sha256 solstone-journal-<version>-linux-x86_64.sha256 \
              --release solstone-journal-<version>-linux-x86_64.release
```

With no `--prefix` it installs under `~/.local/solstone-journal`, keeps each version in its own directory, and points a `current` symlink at the live one. It adds `current/bin` to PATH by writing a block into `~/.profile` between `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`. `--no-path` skips that edit, so a throwaway or side-by-side prefix does not touch your login files. On success it prints the version, the prefix, and how to pick up PATH.

⚠ **`~/.profile` is read by login shells.** A new terminal window on most linux desktops is not one, and zsh does not read it at all. Either log out and back in, or:

```bash
. ~/.profile
```

### A distribution package

`apt` and `dnf` do not check our signature. Run this first, then install:

```bash
minisign -Vm solstone-journal-<version>-linux-x86_64.manifest.json \
  -p solstone-journal-release.pub
```

On Debian or Ubuntu:

```bash
sudo apt install ./solstone-journal-<version>-linux-x86_64.deb
```

On Fedora or RHEL:

```bash
sudo dnf install ./solstone-journal-<version>-linux-x86_64.rpm
```

Either one puts `solstone` and `journal` on PATH for every account on the machine.

### One tree, whichever machine

There is no separate download for talking to a journal running elsewhere. The tree carries `solstone` alongside the journal binaries, so one install covers both roles. You carry a few binaries you will not run, and nothing else changes.

## Install the journal on mac

Apple Silicon only. `install.sh` refuses any other mac by name.

⚠ **The tree is not published yet.** Same origin and same bootstrap as linux, above. Until the first release lands on `updates.solstone.app`, start from the files you have. `install.sh` lives in this repository at `core/distribution/install.sh`.

Every release names its files `solstone-journal-<version>-macos-arm64`. The two containers are a `.tar.gz` and a signed, notarized, stapled `.pkg`. Each release also carries a `.sha256`, a `.manifest.json`, a `.manifest.json.minisig`, a `.release` record, and a `.signing.json`.

### The archive

This is the route to run. It does not need administrator rights and it does not write `/usr/local`. Run this first, then install. The minisign check is a step you take; it does not replace Apple's signature on the `.pkg`.

```bash
minisign -Vm solstone-journal-<version>-macos-arm64.manifest.json \
  -p solstone-journal-release.pub
sh core/distribution/install.sh \
  --archive solstone-journal-<version>-macos-arm64.tar.gz \
  --sha256 solstone-journal-<version>-macos-arm64.sha256 \
  --release solstone-journal-<version>-macos-arm64.release
```

With no `--prefix` it installs under `~/.local/solstone-journal` and points a `current` symlink at the live version. On mac it writes the PATH block to both `~/.zprofile` (zsh, the login shell) and `~/.profile`. `--no-path` skips that edit, so a throwaway or side-by-side prefix does not touch your login files. On success it prints the version, the prefix, and how to pick up PATH.

macos logs you into zsh, which never reads `~/.profile`. Open a new terminal, or:

```bash
. ~/.zprofile
journal --version
```

**To verify the archive by hand:** `install.sh` already does this for you. macos has no `sha256sum`; use:

```bash
shasum -a 256 -c solstone-journal-<version>-macos-arm64.sha256
```

The checksum file carries one line for each container. If you only have the tarball, the extra `.pkg` line will complain; that is the sidecar, not a failed digest.

### The package

The `.pkg` is the `/usr/local` route: same tree, signed with Developer ID Installer, notarized, and stapled. `/usr/local/bin` is already on the default PATH via `/etc/paths`. Apple's signature is that chain. Our minisign check is a step you take; `installer` does not run it.

```bash
minisign -Vm solstone-journal-<version>-macos-arm64.manifest.json \
  -p solstone-journal-release.pub
sudo installer -pkg solstone-journal-<version>-macos-arm64.pkg -target /
```

That writes the live system prefix. Do not run it on a machine whose `/usr/local` you are not ready to change.

The solstone app on your mac still installs from its own signed bundle, under [install the solstone app on your devices](#install-the-solstone-app-on-your-devices). That is a different package from the journal.

## Set up

```bash
journal setup
```

This runs the setup readiness doctor battery and confirms the journal directory at `~/journal`. It fetches the local transcription model (~1 GB), installs the `solstone` skill for Claude Code, Codex, and Gemini, and installs the journal-side `solstone` and `journal` router skills so journal agents can help tend the journal. It then starts a background service (`systemd` on linux, `launchd` on mac at `~/Library/LaunchAgents/org.solpbc.solstone.plist`) listening on http://localhost:5015. The default port is shared across logins. A second journal on that port, including one started under another login, cannot bind it.

Let your human know: **open http://localhost:5015 in a browser**. The first-run wizard walks them through setting their identity and choosing a provider.

⚠ **The tree carries the binaries the journal needs to run, not the transcription stack.** The Parakeet transcription helper and its model are fetched during setup, by `journal install-models`. `journal doctor --readiness` runs the actual binary before reporting it ready, and on linux it gives the exact package-manager command when the system OpenMP runtime listed in prerequisites is missing.

`journal doctor` reports whether the transcription runtime, the native speaker-analysis helper, and the models they need are ready.

The linux local model provider picks its own GPU backend. On RTX 30, 40 and 50 series NVIDIA GPUs with a CUDA 13 driver it runs natively on CUDA, and the runtime downloads from `updates.solstone.app` as a checksum-pinned artifact. Every other hardware GPU uses Vulkan. CPU and software Vulkan devices are rejected rather than falling back silently. Transcription runs on the CPU runtime when the GPU cannot hold both it and the model.

If the service fails to start, check `journal service logs`.

## Choosing a provider

Choose a provider in settings → providers. The available paths have different hardware needs and data flows.

- **local built-in, the default.** a capable setup needs **6 GB of GPU memory** on linux, or a **16 GB Apple Silicon mac** (the model is ~3.4 GB on disk, plus the ~1 GB transcription model). The `solstone check` command checks first and tells you what will not fit; on linux it also needs a supported hardware GPU (see [set up](#set-up)).
- **an engine you bring yourself**, if your machine cannot clear that bar or you would rather not spend its power. Configure the solstone app with Google (Gemini), OpenAI, or Anthropic using **your own developer API key**, created in that provider's developer console, *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). You can also configure it with your own endpoint instead of a cloud provider: a model you run yourself, on this machine or another one you control. You can switch any time in settings → providers.
- **confidential processing**, if you would rather not run a provider yourself. Available to approved scouts. Your journal must verify the service before material leaves; if it cannot verify the service, the material stays in your journal. See [what material reaches your AI provider](DATA-FLOW.md) for the full conditions and data flow.

For the full picture of what is sent, to whom, and under whose terms, see [what material reaches your AI provider](DATA-FLOW.md).

## Install the solstone app on your devices

Your journal works alongside the solstone app: the app takes in what you share with it, and all of it goes into your journal. Each platform ships its own package; install one for each machine where you want the solstone app.

⚠ Each of these has its own install guide and they are the current source of truth. The pip and pipx routes they used to document are legacy builds that no longer receive releases.

**mac:** download the signed app bundle from https://solstone.app/download and drag it to Applications. On first launch it finds the journal running on this machine; click **connect your journal** to pair.

**linux:** Follow `solstone-linux`'s own INSTALL guide. Installing the service and pairing it are two separate steps. `install-service` writes and starts the unit, and pairing reads a pair link you create in the journal.

**tmux terminal sessions:** follow `solstone-tmux`'s own INSTALL guide, which also carries the steps for retiring a previous Python installation.

## Moving from a pip, uv or pipx install

**Your journal itself is untouched by any of this.** It is a folder of dated directories and no installer owns it.

Earlier releases installed the journal as a set of Python packages (`pip`, `uv tool`, or `pipx`). Install the tree as above, then run:

```bash
journal setup
```

That single command finds a real prior install — its `solstone`, `journal`, and `sol` binaries wherever `pip`/`uv`/`pipx` put them under `~/.local/bin` — stops its service, and replaces it, automatically, in one invocation. There is no separate cleanup command to run first. Running `pip uninstall` / `uv tool uninstall` / `pipx uninstall`, or `journal service stop` against the old install, yourself before running `journal setup` only removes the evidence setup needs to find and safely replace it; let `journal setup` do it.

Setup keeps a durable backup of anything it replaces under `~/.local/share/solstone/setup-backups/` before touching it, so nothing is destroyed outright.

⚠ **There is no CUDA build of the tree.** If you were on `solstone-journal-cuda`, transcription moves to the CPU runtime. It uses the same model on the CPU, so long recordings take longer to process; nothing else about them changes. The local *model* provider still uses your GPU where it can. That path is separate and is described under [set up](#set-up).

## Upgrading

Install the newer package or archive the same way you installed the first one, then run `journal setup`. The setup step refreshes runtime artifacts and reconciles the service unit if anything has changed.

The archive route keeps each version in its own directory and moves the `current` symlink, so an upgrade unpacks a second tree alongside the live one before switching.

### If you already have a journal with history in it

A few more things happen, or need to happen, on top of the install.

**Search index rebuild, on an older index schema.** An index on the older schema is dropped and rebuilt on first open after upgrade. The rebuild usually queues itself automatically, but if the service was still starting up when that happened, it can miss the window and print a message asking you to run it yourself. If search feels empty, or noticeably thinner than your journal's actual history, right after upgrading, run:

```bash
journal indexer --rescan-full
```

This is a full historical rescan and can take a while on a large journal.

**connections/edges backfill.** The relationship layer between entities (who is connected to whom, and how) is derived by a separate pass, and extraction is incremental on file modification time. Once a day’s mtimes are recorded, an ordinary rescan will not re-extract its edges even with `--rescan-full`. A weekly schedule rebuilds them on its own. To force it now:

```bash
journal indexer --rebuild-edges
```

Run this if your existing days show no connections and you would rather not wait for the weekly pass.

**if your journal isn't at the default location.** `journal setup` expects `~/journal` unless told otherwise. If your journal lives somewhere else, point setup at it explicitly so it reuses that journal instead of creating an empty one at the default path:

```bash
journal setup --journal /path/to/your/journal --accept-existing-journal
```

With no `--journal`, setup takes `SOLSTONE_JOURNAL`, then the `journal` key in `~/.config/solstone/config.toml`, then `~/journal`. On a machine with no prior config that last step starts fresh, and a fresh `~/journal` looks like a working install even though your actual history is untouched at the old path.

## Uninstall

**None of this removes your journal.** It is a folder of dated directories and it survives every step below.

1. Remove setup-managed runtime files: `journal setup --clean-uninstall --yes`
   this removes the service unit, the managed `solstone` and `journal` wrappers in `~/.local/bin`, its config, and the setup manifest. Without `--yes` it asks first; in a non-interactive shell that form refuses and exits 2, and nothing is removed. If the service cannot be removed, uninstall stops there and leaves the wrappers in place so you still have `journal` to retry.
2. Optional: remove the installed `solstone` agent skill: `solstone skills uninstall`.
3. Remove the tree, by the route you installed it:
   - `sudo apt remove solstone-journal` or `sudo dnf remove solstone-journal`
   - Archive install: delete the prefix directory (`~/.local/solstone-journal` by default) and the PATH block `install.sh` added to `~/.profile` (and on mac, `~/.zprofile`), marked with `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`.
   - Mac `.pkg` install: use the receipt the installer registered. It names every file the
     package put on disk.

     ⚠ **do not pipe `pkgutil --files` into `rm -rf`.** it lists directories too (`bin`, `lib`,
     `share`), and `/usr/local` is shared with Homebrew, Docker, VS Code and anything else on this
     machine. A raw `rm -rf` over the full list takes all of it with the package.

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

     The first loop removes only the files the package installed. Nothing else in `/usr/local` is
     touched. The second removes the directories it created, deepest first, and only the ones that
     are empty afterward; `rmdir` refuses a directory that still holds something else, so `bin`
     itself (shared with other tools) is left standing. the third clears the receipt.
4. Mac only: drag `/Applications/solstone.app` to Trash.
5. Mac only, optional: remove the solstone app's data and the Parakeet model cache:
   ```bash
   rm -rf ~/Library/Application\ Support/solstone/
   ```
   This evicts the Parakeet cache; reinstall will re-download it.
6. Mac only, optional: reset privacy permissions:
   ```bash
   tccutil reset Microphone app.solstone.observer
   tccutil reset ScreenCapture app.solstone.observer
   ```
   Or use System Settings → Privacy & Security.

## Done

Once it is running, the solstone app takes in what you share with it and all of it goes into your journal. Conversations are transcribed, people and projects are surfaced, a knowledge graph is built, and everything is searchable at http://localhost:5015. Your journal is one folder per day on a device you own. See [what material reaches your AI provider](DATA-FLOW.md) for material that leaves it.

Source code: https://github.com/solpbc/solstone-journal
company: https://solpbc.org

## Feedback

Questions, feedback, or a bug? **Follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone-journal/issues for bugs, or reach support at https://support.solstone.app. You do not need to know anyone. Those are the front doors.

(running into trouble or want to develop on solstone yourself? See [CONTRIBUTING.md](CONTRIBUTING.md).)
