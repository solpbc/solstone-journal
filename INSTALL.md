# Installing solstone

These instructions are for a coding agent and human working together. solstone is a personal memory platform: the solstone app takes in what you share with it, and all of it goes into your journal. Your journal is always private, only yours. It lives on a device you own; see [what material reaches your AI provider](DATA-FLOW.md) for material that leaves it. Open source, made by sol pbc.

**supported platforms:** linux, macos 15 or later on Apple Silicon, and windows 10 (22H2) or 11 on x64, or windows 11 on arm64. On mac, the journal app is the only supported way to run the journal. On windows, the journal has its own installer.

The latest version of these instructions is at https://solstone.app/install.

## Before you begin

### Check whether solstone is already installed on linux

```bash
solstone --version 2>&1 && solstone journal service status 2>&1
```

On linux, a missing PATH command can also mean an archive installation used `--no-path`; check its prefix before installing again. On mac, check for `/Applications/journal.app` instead. On windows, run `solstone journal --version` in a terminal; if windows cannot find `solstone`, see [install on windows](#install-on-windows).
On linux, if both commands succeed and the second command reports `state: running (systemd)`, skip to [install the solstone app on your devices](#install-the-solstone-app-on-your-devices).

For a curl installation, the prefix is `~/.local`, with binary `~/.local/current/bin/solstone` and receipt `~/.local/install-receipt`. For an archive installation, run the linux `solstone` commands below through that installation's `current/bin/solstone`. The default full path is `~/.local/solstone-journal/current/bin/solstone` (receipt `~/.local/solstone-journal/install-receipt`); use your chosen prefix if it differs. This also covers `--no-path` installations.

### Prerequisites

The journal needs glibc 2.34 or newer and the GCC 11 C++ runtime. Ubuntu 22.04, Debian 12, RHEL 9, Fedora 35 and newer releases of each include both. Local transcription also needs the GCC 12 C++ runtime, which Ubuntu 22.04, Debian 12 and Fedora 36 or newer include and RHEL 9 does not.

The installer refuses without `minisign` and the OpenMP runtime (`libgomp.so.1`):

```bash
sudo apt install minisign libgomp1
sudo dnf install minisign libgomp
sudo pacman -S minisign gcc-libs
```

## Install the journal on linux

⚠ **linux only.** the tree is built for `linux-x86_64` and `linux-aarch64`, and the bootstrap refuses any other system. For mac, see [install on mac](#install-on-mac).

### Where the files come from

The release channel is `updates.solstone.app`. The main installer lives in the [solstone repository](https://github.com/solpbc/solstone) and is served at `https://solstone.app/install.sh`. On linux it verifies the platform release, then delegates journal setup to the signed bootstrap versioned with that release. The journal bootstrap lives in this repository at `core/distribution/install.sh`; its compatibility URL is `https://updates.solstone.app/solstone-journal/install.sh`, and it refuses macos.

One command does the whole thing, including signed-manifest verification and `solstone journal setup`. For a human at a terminal:

```bash
curl -fsSL https://solstone.app/install.sh | sh
```

Pressing Enter on a terminal selects the journal. With no terminal and no component, the installer stops and asks you to name one with `--components`. Agents and non-interactive runs use:

```bash
curl -fsSL https://solstone.app/install.sh | sh -s -- --components journal
```

The curl route installs under prefix `~/.local`, with binary `~/.local/current/bin/solstone` and receipt `~/.local/install-receipt`.

That follows the `release` lane's `latest` pointer, fetches the archive and signed release set from `updates.solstone.app`, verifies the manifest signature with the pinned minisign key, then checks the manifest digests for the selected archive, checksum, and release record. It installs and runs setup once. When a desktop is available, the installer opens the setup page; otherwise it prints the link and an SSH tunnel command for opening it from another computer. After setup, use the printed network-page link to make a pairing link for the solstone app. Pass `--version <version>` to pin a version instead of following `latest`. A tree downgrade with `--version` proceeds only when that exact build is still inside this release's three-directory `journal-v2` window; outside the epoch or window it refuses without touching the journal. The archive route below is the same operation with the files already on disk.

Every release names its files the same way: `solstone-journal-<version>-linux-<arch>`, where `<arch>` is `x86_64` or `aarch64`. The three archives are `.tar.gz`, `.deb` and `.rpm`. Each release also carries a `.sha256`, a `.manifest.json`, a `.manifest.json.minisig` and a `.release` record.

### Verify independently

One minisign signature covers every archive in the set. `install.sh` verifies it with the product key pinned in the script. Keep this independent check when you want to verify the release before running any installer; `apt` and `dnf` do not perform it. Replace `<arch>` with `x86_64` or `aarch64` for your machine.

```bash
minisign -Vm solstone-journal-<version>-linux-<arch>.manifest.json \
  -p solstone-journal-release.pub
sha256sum solstone-journal-<version>-linux-<arch>.tar.gz  # or the .deb or .rpm
```

If minisign refuses, stop. Compare the artifact digest printed by `sha256sum` with that artifact's exact entry under `files` in the signed manifest. A matching manifest signature without this digest comparison does not authenticate the package or archive you are about to run.

The public key is in this repository at `packaging/keys/solstone-journal-release.pub`. It is also at `https://updates.solstone.app/solstone-journal/minisign.pub`. Install minisign from your distribution if you do not have it (for example `apt install minisign`, `dnf install minisign` or `pacman -S minisign`).

If `minisign` is absent, `install.sh` refuses before changing anything and prints the command that adds it on apt, dnf, pacman and zypper systems, with EPEL first on AlmaLinux and Rocky. `--skip-signature` is the explicit opt-out, and the install receipt records that verification was skipped.

### The archive

This local-file route verifies the manifest signature, then checks its digests for the selected archive, checksum, and release record before installing. Give it all five files:

```bash
sh core/distribution/install.sh --archive solstone-journal-<version>-linux-<arch>.tar.gz \
              --sha256 solstone-journal-<version>-linux-<arch>.sha256 \
              --release solstone-journal-<version>-linux-<arch>.release \
              --manifest solstone-journal-<version>-linux-<arch>.manifest.json \
              --minisig solstone-journal-<version>-linux-<arch>.manifest.json.minisig
```

With no `--prefix` it installs under `~/.local/solstone-journal`, keeps each version in its own directory, points `current` at the live one, and runs `solstone journal setup --yes` from that build so the managed PATH wrapper and service follow it. It leaves a plain-text receipt at `~/.local/solstone-journal/install-receipt`. It adds `current/bin` to PATH by writing a block into `~/.profile` between `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`. `--no-path` skips that edit, so a throwaway or side-by-side prefix does not touch your login files. On success it prints the version, lane, prefix, and how to pick up PATH.

Options:
- `--role <journal|cli>`: installation role (default: `journal`). `journal` configures host background services, managed wrappers, and journal state via `solstone journal setup`. `cli` installs the verified tree binaries and PATH integration only, without configuring or running background services (`solstone journal setup` is never invoked, `setup_status=not-applicable`, `service_policy=none`). When transitioning a prefix previously configured to run a journal to `--role cli`, run `solstone journal setup --clean-uninstall` first (which removes setup/service ownership without deleting journal records or the verified payload tree).
- `--no-start`: during journal installation, skips starting the background supervisor (`solstone journal setup --skip-service`), marking `service_policy=no-start` in the receipt while completing all other setup tasks. Has no effect on `--role cli`.

`--prune` is a separate, explicit maintenance run. It keeps `current` plus the two newest other version directories; an install or upgrade never prunes as a side effect.

⚠ **`~/.profile` is read by login shells.** A new terminal window on most linux desktops is not one, and zsh does not read it at all. Either log out and back in, or:

```bash
. ~/.profile
```

### A distribution package

`apt` and `dnf` do not check our signature. Complete the signature and digest comparison under [verify independently](#verify-independently) first, then install the artifact you checked.

```bash
sudo apt install ./solstone-journal-<version>-linux-<arch>.deb
```

On Fedora or RHEL:

```bash
sudo dnf install ./solstone-journal-<version>-linux-<arch>.rpm
```

Either one puts `solstone` on PATH for every account on the machine and installs the OpenMP runtime dependency. `journal <args>` has moved to `solstone journal <args>`; update any commands or scripts that use the old name. Run `solstone journal setup`; that owner-scoped step writes `~/.local/share/solstone/package-install-receipt` because the packages intentionally have no maintainer scripts.

On linux, `solstone update` updates copies recorded by the solstone installer. `solstone update --check` reports whether an update is available. If the installation cannot be identified, use its original installer.

### One tree, whichever machine

There is no separate download for talking to a journal running elsewhere. The tree carries `solstone` alongside the journal binaries, so one install covers both roles. You carry a few binaries you will not run, and nothing else changes.

## Install on mac

Apple Silicon and macos 15 or later are required. The journal app is the only supported way to run the journal on a mac. Download it from [solstone.app/download/journal](https://solstone.app/download/journal), move it to Applications, and open it. First run helps you choose your journal's mark and location.

To install the journal app from the terminal:

```bash
curl -fsSL https://solstone.app/install.sh | sh -s -- --components journal
```

Use `--components all` to install both the journal app and the solstone app. The installer verifies the signed, notarized app bundles and puts them in `/Applications`. Each app handles its own updates after that. The low-level journal bootstrap in this repository refuses macos so it cannot create a second runtime, PATH wrapper, or launchd service.

### The journal command line on a mac

The journal app doesn't put a `journal` or `solstone` command on your PATH. When you want the command line, choose **open admin terminal** from the journal app's journal menu. It opens a Terminal window in your own shell where `solstone` runs the app's own copy. `journal <args>` has moved to `solstone journal <args>`; update any commands or scripts that use the old name. In zsh (the mac default), bash or fish, that holds even if another `journal` is earlier on your PATH. Nothing is installed, and your shell files stay as they were.

Scripts and agents that can't open that window can call the app's copy by its full path:

```bash
/Applications/journal.app/Contents/Resources/solstone-runtime/bin/solstone journal --version
```

If the app is somewhere other than `/Applications`, use that location instead.

If you have an older command-line journal installation, install the journal app and open it. The app adopts the existing journal only when it can verify the installation it is taking over. If anything is unclear, it stops and tells you what needs attention. Your journal stays where it is. See the [mac migration guidance](https://solstone.app/install#macos-migration) or [contact support](https://support.solstone.app) if you need a hand.

## Install on windows

windows 10 (22H2) or 11 on an Intel or AMD (x64) computer, or windows 11 on an arm64 computer such as a Snapdragon X PC. Settings › System › About shows which you have, under System type (x64-based or ARM-based). No administrator access is needed: the journal installs for you alone, under `%LOCALAPPDATA%\SolstoneJournal`, and it is separate from the solstone app for windows.

1. Download the journal installer and run it: [solstone.app/download/journal/windows/latest](https://solstone.app/download/journal/windows/latest) on x64, or [solstone.app/download/journal/windows/arm64/latest](https://solstone.app/download/journal/windows/arm64/latest) on arm64. It is signed by sol pbc.
2. Open **journal** from the Start menu and follow its first-run steps. It sets up your journal (`%USERPROFILE%\journal` by default) and starts it in the background. It starts again each time you sign in.

For command-line diagnostics after that, open a new terminal window so it picks up the `solstone` command the installer added. If you need to select an existing journal, use the location guidance below.

The journal starts closed to devices on your network, so windows doesn't ask about the network when the journal first starts. To continue in the solstone app on this computer, use the journal window. You can still pair from the journal's network page: choose "pair a device", then "pair the solstone app on this computer". Your journal stays closed. To pair a phone or another computer over your own network, choose "open to devices on your network" on that page, or run `solstone call link local-network open`. windows may then ask whether **journal**, from sol pbc, can use public and private networks, and on a standard account an administrator has to allow it. If windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking **journal**: an administrator can allow it in Windows Security, under Firewall & network protection › Allow an app through firewall.

### If something doesn't work on windows

These commands show what happened. They only read, so they are safe to run at any time:

```powershell
solstone journal check
solstone journal service logs
solstone call thinking local bootstrap-status
solstone journal health logs --since 1d --service solstone-core-exe -c 200
```

`check` says whether this computer can run the bundled local models. `service logs` shows the journal's recent output. `bootstrap-status` shows the last local thinking install and the reason it stopped, if it did, and the last command shows the installer's log from the past day. `service logs` works on windows in journal releases after 2.0.38. If you [contact support](https://support.solstone.app), include what they print.

### Verify independently on windows

Each release publishes a checksum file beside the installer, at `https://updates.solstone.app/solstone-journal/release/windows/solstone-journal-<version>-windows-<arch>.sha256`, where `<arch>` is `x86_64` or `aarch64` (arm64). Before you run anything, compare the hash `Get-FileHash` prints with the one in that file:

```powershell
Get-FileHash .\solstone-journal-<version>-windows-<arch>-setup.exe -Algorithm SHA256
Get-AuthenticodeSignature .\solstone-journal-<version>-windows-<arch>-setup.exe | Format-List Status, SignerCertificate
```

The status should read `Valid`, and the signer certificate's subject should start with `CN=sol pbc`. Every file in the journal's program folder is listed in a manifest signed with the same release key as the linux and mac releases. Before it uses its bundled tools and models, the journal checks the program folder against that manifest and refuses them if anything does not match.

## Finish setup on linux

The installer above already runs journal setup. Follow the setup page it prints; do not run setup a second time. If you installed a `.deb` or `.rpm` directly, or are repairing setup, run:

```bash
solstone journal setup
```

This runs the setup readiness doctor battery and sets up your selected journal (`~/journal` by default). It fetches the local transcription model (~1 GB), installs the `solstone` skill for Claude Code, Codex, and Gemini when their configuration folders are already present, and installs the journal-side `solstone` and `journal` router skills so your agents can help tend your journal. It then starts a systemd user service listening on http://localhost:5015. The default port is shared across logins. A second journal on that port, including one started under another login, cannot bind it.

Let your human know: **open http://localhost:5015/init in a browser on the journal's computer**. The first-run wizard walks them through setting their identity and choosing a provider. If the computer has no browser, use the SSH command the installer prints and open the setup page from the other computer; it forwards the journal and the agent door.

⚠ **The tree carries the binaries the journal needs to run, not the transcription stack.** The Parakeet transcription helper and its model are fetched during setup, by `solstone journal install-models`. `solstone journal doctor --readiness` runs the actual binary before reporting it ready, and on linux it gives the exact package-manager command when the system OpenMP runtime is missing.

`solstone journal doctor` reports whether the transcription runtime, the native speaker-analysis helper, and the models they need are ready.

The linux local model provider picks its own GPU backend. On RTX 30, 40 and 50 series NVIDIA GPUs with a CUDA 13 driver it runs natively on CUDA, and the runtime downloads from `updates.solstone.app` as a checksum-pinned artifact. Every other hardware GPU uses Vulkan. CPU and software Vulkan devices are rejected rather than falling back silently. Transcription runs on the CPU runtime when the GPU cannot hold both it and the model.

If the service fails to start, check `solstone journal service logs`.

## Choosing a provider

Choose a provider in the journal's thinking app. The available paths have different hardware needs and data flows.

- **local built-in, the default.** a capable setup needs **6 GB of GPU memory** on linux or windows, or a **16 GB Apple Silicon mac**. The `solstone journal check` command checks first and tells you what will not fit; on linux it also needs a supported hardware GPU (see [set up on linux](#finish-setup-on-linux)).

  On windows, the local built-in model needs a compatible GPU with at least **6 GB of GPU memory**. On arm64 windows computers it hasn't been tested on a GPU. A new setup leaves thinking unchosen if the GPU has less or its memory cannot be checked. A local install refuses before downloading and shows the reason on the thinking page. You can choose a model you bring yourself or confidential processing. on mac and linux, a local install also stops before downloading when the computer is known to be below the bar or has no usable GPU; if the computer's memory can't be checked, installation still starts and the thinking page keeps a warning for that attempt.
- **a model you bring yourself**, if your machine cannot clear that bar or you would rather not spend its power. Configure Google (Gemini), OpenAI, or Anthropic in your journal's thinking app using **your own developer API key**, created in that provider's developer console, *not* the consumer chat product (gemini.google.com / chatgpt.com / claude.ai). You can also configure it with your own endpoint instead of a cloud provider: a model you run yourself, on this machine or another one you control. You can switch any time in the thinking app.

- **confidential processing**, if you would rather not run a provider yourself. It is off until you turn it on. While it is active, your journal verifies the service before material leaves; if it cannot verify the service, the material stays in your journal. See [what material reaches your AI provider](DATA-FLOW.md) for the full conditions and data flow.

For the full picture of what is sent, to whom, and under whose terms, see [what material reaches your AI provider](DATA-FLOW.md).

## Install the solstone app on your devices

Your journal works alongside the solstone app: the app takes in what you share with it, and all of it goes into your journal. Each platform ships its own package; install one for each machine where you want the solstone app.

⚠ Each of these has its own install guide and they are the current source of truth. The pip and pipx routes they used to document are legacy builds that no longer receive releases.

**mac:** download the signed app bundle from https://solstone.app/download and drag it to Applications. On first launch it finds the journal running on this machine; click **connect your journal** to pair.

**linux:** Follow `solstone-linux`'s own INSTALL guide. Installing the service and pairing it are two separate steps. `install-service` writes and starts the unit, and pairing reads a pair link you create in the journal.

**tmux terminal sessions:** follow `solstone-tmux`'s own INSTALL guide, which also carries the steps for retiring a previous Python installation.

## Moving from a pip, uv or pipx install on linux

Earlier releases installed the journal as a set of Python packages (`pip`, `uv tool`, or `pipx`). If you are moving specifically from v1.0.22 after installing this linux native `.deb` or `.rpm`, run this one time instead, even when `~/.local/bin/journal` comes first on your normal PATH:

```bash
/usr/bin/solstone journal setup
```

This exception is only for that v1.0.22 linux package crossover. Do not remove the old install first: setup recognizes its runtime and service artifacts, preserves your journal, replaces only what it can identify, and saves recovery backups of recognized legacy launchers. For every other install route, run:

```bash
solstone journal setup
```

That one setup command finds a real prior install, its `solstone`, `journal`, and `sol` binaries wherever `pip`/`uv`/`pipx` put them under `~/.local/bin`, stops its service, and replaces it automatically in one invocation. There is no separate cleanup command to run first. Running `pip uninstall` / `uv tool uninstall` / `pipx uninstall`, or `solstone journal service stop` against the old install, yourself before setup only removes the evidence it needs to find and safely replace the old runtime; let the applicable setup command above do it.

When setup replaces recognized legacy launchers, it keeps durable recovery backups under `~/.local/share/solstone/setup-backups/` before touching those launchers.

⚠ **There is no CUDA build of the tree.** If you were on `solstone-journal-cuda`, transcription moves to the CPU runtime. It uses the same model on the CPU, so long recordings take longer to process; nothing else about them changes. The local *model* provider still uses your GPU where it can. That path is separate and is described under [set up on linux](#finish-setup-on-linux).

## Upgrading

On windows, download and run the newer installer. It updates the journal in place, and a running journal starts again afterward.

On mac, each app handles its own updates. The shell installer verifies an existing app and leaves it unchanged; it does not replace or upgrade app bundles.

On linux, use the route that owns the installation:

For a tree install, `sh install.sh --upgrade` is the whole upgrade: it preserves the recorded lane unless `--lane` is explicit, verifies the signed release, flips `current`, and repoints the managed wrapper and service through setup. If a package owns the install, the tree installer refuses. This release has no package repository. To upgrade an existing 2.x package installation, download the newer local `.deb` or `.rpm` and repeat the applicable package install command above. You do not need to run setup again.

The new build takes effect when your journal next starts, including after a login or reboot. It checks for updates to models, location data and agent skills you already use. Files that already match the new build stay in place, and optional assets you have not installed stay absent. A failed refresh does not stop your journal; it tries again on a later start.

If a tree install reports `setup-failed`, follow the named correction and rerun the same `install.sh` command. When setup may already have changed a managed wrapper or service, the installer keeps the candidate selected and writes `setup_status=pending` in its receipt; the same command finishes that transaction safely.

The archive route keeps each version in its own directory and moves the `current` symlink, so an upgrade unpacks a second tree alongside the live one before switching.

### If you already have a journal with history in it

A few more things happen, or need to happen, on top of the install.

**Search index rebuild for the v1→v2 crossing only.** There is no written-schema divergence yet within the 2.x line. An index on the v1 schema is dropped and rebuilt on first open after that crossing. The rebuild usually queues itself automatically, but if the service was still starting up when that happened, it can miss the window and print a message asking you to run it yourself. If search feels empty, or noticeably thinner than your journal's actual history, right after that crossing, run:

```bash
solstone journal indexer --rescan-full
```

This is a full historical rescan and can take a while on a large journal.

**connections/edges backfill.** The relationship layer between entities (who is connected to whom, and how) is derived by a separate pass, and extraction is incremental on file modification time. Once a day’s mtimes are recorded, an ordinary rescan will not re-extract its edges even with `--rescan-full`. A weekly schedule rebuilds them on its own. To force it now:

```bash
solstone journal indexer --rebuild-edges
```

Run this if your existing days show no connections and you would rather not wait for the weekly pass.

**if your journal isn't at the default location.** To select an existing journal explicitly, use:

```bash
solstone journal setup --journal /path/to/your/journal --accept-existing-journal
```

Setup keeps the journal in an `adopted` installation record unless you pass `--journal` or set `SOLSTONE_JOURNAL`. If setup was interrupted with a `prepared` record, it resumes that record's journal even with a different explicit location. For a new installation or a `tombstoned` record, setup uses `--journal`, then `SOLSTONE_JOURNAL`, then the `journal` key in `~/.config/solstone/config.toml`, then `~/journal`. For a windows `prepared` record that names a journal inside the program folder, move that journal outside the folder first, then explicitly select its existing new location with `--accept-existing-journal`; setup refuses this recovery while the old path still exists. If setup falls back to `~/journal`, it can create a fresh journal there while your history stays at the old path.

## Uninstall on linux

Your journal stays, including its models, keys, device connections and setup record.

If an archive was installed only with `--role cli` and setup was never run, skip to step 2.

1. Before removing a journal installation, run its setup cleanup. For a package installation, use `solstone journal setup --clean-uninstall --yes`. For a curl installation, use `~/.local/current/bin/solstone journal setup --clean-uninstall --yes`. For an archive, use `~/.local/solstone-journal/current/bin/solstone journal setup --clean-uninstall --yes`, replacing the default prefix if you chose another. This removes its service and owned command wrappers. For the last installation, it also removes the journal-location config, setup backups, package-install receipt and rclone cache outside the journal. It prints where your journal remains and skips any cleanup directory containing it. If cleanup refuses or any step fails, stop before removing the program so you can retry through the same command.
2. Remove the program through the route that installed it: `sudo apt remove solstone-journal`, `sudo dnf remove solstone-journal`, or remove your archive installation prefix. Removing a curl installation removes its `current` link, its version directories, and `install-receipt` under `~/.local`, and leaves the rest of `~/.local`. Check that the prefix does not contain your journal before deleting it. The archive installer's PATH block in `~/.profile`, marked `# BEGIN solstone-journal PATH` and `# END solstone-journal PATH`, can then be removed.

Small installation records stay to prevent old launchers from starting a removed installation. For the last installation, cleanup removes agent skill copies that match this runtime from `~/.claude/skills/solstone`, `~/.codex/skills/solstone` and `~/.gemini/skills/solstone`. Modified or unrecognized copies stay; older links pointing into this runtime are removed. The shared `~/.local/bin` PATH lines stay because other commands may use them.

The solstone app is separate. Run `solstone-linux uninstall-service` before removing its package. Its preferences and pairing remain in `~/.config/solstone-linux`, and local app data remains in `~/.local/share/solstone-linux`.

## Uninstall on windows

Setup requires your journal to be outside the program installation folder, normally `%LOCALAPPDATA%\SolstoneJournal`. Reinstalling with Setup or uninstalling through Settings deletes that folder, including any journal placed inside it. If an older installation has your journal there, move it before reinstalling or uninstalling:

1. Quit the journal app. If setup previously completed, run `solstone journal service stop`, then `solstone journal service uninstall` and wait for it to succeed. This removes its old background registration so setup can use the moved folder.
2. Move the whole journal folder outside the program folder.
3. Select the moved journal with `solstone journal setup --accept-existing-journal --journal "D:\journal"`, replacing `D:\journal` with its new location. Always name that location: otherwise setup can fall back to `~/journal` and create a fresh journal while your history stays in the moved folder.

Keep the moved folder. Once it is outside the program folder, these reinstall and uninstall routes leave it in place. An ordinary in-app update replaces only the program's `current` folder; rerunning Setup replaces the whole program folder.

Open Settings → Apps → Installed apps, find **journal**, and choose Uninstall. Settings removes the program. When the recorded installation can be verified and its cleanup succeeds, that cleanup also removes the background task, sign-in resume entry and command PATH entries. For the last installation, it removes the journal-location config, task profiles, app preferences, mark icon and WebView data outside your journal. WebView data stays if that folder contains your journal. If cleanup refuses or fails, some outside state can remain.

Your journal stays, including its models, keys, device connections and setup record. Small installation records stay under `%LOCALAPPDATA%\solstone-journal\installation-identity` to prevent old launchers from starting a removed installation. On reinstall, select your existing journal again if it is away from the default location.

For manual setup cleanup while keeping the program installed, run `solstone journal setup --clean-uninstall --yes`.

The solstone app has its own Settings entry and `%LOCALAPPDATA%\Solstone` folder. Journal removal leaves that app alone; its own uninstall removes that folder.

## Uninstall on mac

Quit journal, then run this command before dragging it to Trash:

```bash
/Applications/journal.app/Contents/MacOS/journal setup --clean-uninstall --yes
```

On success, this unregisters journal's login agent and runs its setup cleanup. For the last installation, cleanup removes the journal-location config and setup backups outside your journal. It prints where the journal remains and skips any cleanup directory containing it. Wait for the command to succeed, then check that your journal is outside `/Applications/journal.app` before dragging the app to Trash.

Your journal stays, including its models, keys, device connections and setup record. Small installation records stay under `~/Library/Application Support/solstone/installation-identity` to prevent old launchers from starting a removed installation. Journal preferences, `~/Library/Application Support/SolstoneJournal` and `~/Library/Application Support/sol/journal-handoff.json` stay. For the last installation, cleanup removes agent skill copies that match this app from `~/.claude/skills/solstone`, `~/.codex/skills/solstone` and `~/.gemini/skills/solstone`. Modified or unrecognized copies stay; older links pointing into this app are removed. Reinstall may ask you to select your journal again.

To remove the separate solstone app, turn off its launch-at-login setting, quit it, and drag `/Applications/solstone.app` to Trash. Its preferences, pairing, local app data and model cache stay, including `~/Library/Application Support/Solstone` and `~/Library/Application Support/solstone`. Its microphone and screen-sharing permissions stay in macos settings. You can reset those separately in System Settings → Privacy & Security.

## Done

Once it is running, the solstone app takes in what you share with it and all of it goes into your journal. Conversations are transcribed, people and projects are surfaced, a knowledge graph is built, and you can search indexed journal content at http://localhost:5015. Your journal is one folder per day on a device you own. See [what material reaches your AI provider](DATA-FLOW.md) for material that leaves it.

Source code: https://github.com/solpbc/solstone-journal
company: https://solpbc.org

## Feedback

Questions, feedback, or a bug? **Follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at https://github.com/solpbc/solstone-journal/issues for bugs, or reach support at https://support.solstone.app. You do not need to know anyone. Those are the front doors.

(running into trouble or want to develop on solstone yourself? See [CONTRIBUTING.md](CONTRIBUTING.md).)

Maintaining the windows package: [winget manifests and channel check](packaging/winget/README.md).
