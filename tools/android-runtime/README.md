# Android journal development runtime

This app runs the journal's storage, indexing and search libraries in an app-owned Android bound service. The activity and worker share an app UID and use separate processes. It is a development app for a dedicated, plugged-in device kept in the foreground.

The package is `app.solstone.development.journal`. It requests Internet and network-state permissions, disables Android backup and uses synthetic data in its own private directory. It has a separate debug signing identity. Uninstalling it deletes its fixtures. Reserve your test device before running the scripts.

## Build and install

Install the repository's pinned Rust toolchain and `aarch64-linux-android` target, JDK 17, Android SDK platform 36, build-tools 35.0.0 and NDK r30. The [Android cross-build instructions](../../docs/ANDROID_CROSS_BUILD.md) provide the pinned NDK acquisition recipe. Set `ANDROID_HOME`, `JAVA_HOME` and `JOURNAL_NDK` to those installations. Cache the Cargo dependencies before the offline build. All native builds require `PARAKEET_LIB_DIR`, `SOLSTONE_FFMPEG_SOURCE_ARCHIVE` and `JOURNAL_ONNX_LIBRARY`, as described below.

From this directory:

```sh
python3 build_native.py
python3 build_apk.py --root "$PWD" --sdk "$ANDROID_HOME" --java "$JAVA_HOME" \
  --debug-keystore /absolute/path/to/retained-debug-keystore.p12
python3 run_device.py full --install --signer-sha256 EXPECTED_SIGNER_SHA256
python3 run_device.py mixed
```

Pass the same retained debug keystore across checkouts; the builder does not create one. Read its signer SHA-256 from `build/artifact-manifest.json`. The installer checks APK bytes, package identity, signer and existing UID before a single streamed install. A signer conflict refuses installation without uninstalling or retrying.

`run_device.py` targets the development handset recorded in its source. Pass `--serial SERIAL` or set `ANDROID_SERIAL` for another bench. It verifies the installed APK hash, wakes the screen and dismisses an unsecured keyguard. It leaves security settings unchanged. the development activity keeps the screen awake while it is visible.

## internal owner build

`--owner-app` packages a launcher named `journal`. it opens the existing journal setup and screens through a private in-app transport. start and stop are in the native bar; health carries processing and service status. a saved stop holds across reopening. the owner variant ignores development command files and keeps the screen's normal sleep behavior.

build native libraries and prepare service tools first. run the current native `solstone-core assets` command and save its JSON catalog. supply that catalog and its pinned Parakeet speech model to the APK builder:

```sh
python3 build_apk.py --root "$PWD" --sdk "$ANDROID_HOME" --java "$JAVA_HOME" \
  --debug-keystore /absolute/path/to/retained-debug-keystore.p12 \
  --owner-app --application-id app.solstone.development.journal.processing \
  --version-code 2026102301 \
  --audio-model /absolute/path/to/tdt-0.6b-v3-q8_0.gguf \
  --asset-catalog /absolute/path/to/assets.json
```

the builder verifies the speech model against the catalog and the checked-in voice models against their runtime pins. the APK includes those files and their notices. on first open, preparation copies missing files into app-private storage before starting the journal. existing model files must match; conflicting bytes are kept and startup refuses. no USB model provisioning is needed for this variant.

install the exact signed APK with the retained signer. open `journal`, choose its mark, and choose thinking. local thinking is unavailable on android; select confidential processing, your own model, or decide later. pair devices from network. for an existing backup, finish initial setup, then use backup to restore it and restart the journal.

use this internal build on a dedicated, plugged-in device with the app in the foreground. it does not promise background hosting. keep the solstone app separate. test a new journal with a separate application ID; preserve the existing package's signer and private data when updating it. this build is not a store release.

## Repeatable checks

`full` exercises 512 synthetic activity records using real `JournalRoot`, atomic writers, the scanner, owner-bound search and SQLite. It requires confirmed create-only publication, existing-destination refusal, a single complete winner among 16 competing publishers, symlink refusal and lock release. It kills and reconnects the worker during an open SQLite transaction and immediately before and after atomic publication. Cancellation is checked between complete native work units; the service's control handler runs separately from its work executor. The activity remains alive during worker failures.

`mixed` runs 360 requests over 30 minutes plus operation time, with five seconds between requests. Its 4,096 records span 16 activity files. Queries alternate with rescans and source replacements. Memory, file descriptors, journal bytes and the activity's 50 ms timer are measured. This is bounded foreground qualification on one fixture and handset; it does not establish unattended operation or a supported device floor.

Additional scenarios are `verify`, `loss`, `exit` and `finish`. `loss` kills the host during work and records the worker's lost reply. Follow each lifecycle scenario with `verify` to reopen and search committed data from cold app processes. Recovery explicitly reconnects and reopens authoritative files. A lost reply leaves the commit outcome unknown until disk state is checked; the app does not retry writes blindly. No persistent job ledger is introduced.

After the runs, force-stop only this package and copy its `files` directory using `adb exec-out run-as app.solstone.development.journal tar -cf - files`. Capture binary stdout directly to a file. Verify that snapshot using a separate host SQLite implementation:

```sh
python3 verify_evidence.py --files files.tar \
  --full evidence/full-<run>/events.txt \
  --mixed evidence/mixed-<run>/events.txt --output verification.json
```

`evidence/` contains actual command exits, app events, APK hashes and device snapshots. `build/` contains the APK and member hashes. Keep the signing key outside the checkout. These and native build outputs are excluded from Git.

## Storage and limits

Android ancestor witnesses use `O_PATH` with directory, no-follow and identity checks. The admitted journal directory retains readable access for synchronization. Bound exclusive file publication uses descriptor-relative `renameat2(RENAME_NOREPLACE)` after staging and file sync; unsupported kernels or filesystems return an error. There is no overwrite-capable fallback. Other Unix platforms retain hard-link publication. Directory sync and final inode/byte observations retain their existing result states.

The native harness enables existing Journal IO test hooks for precise crash boundaries. The SQLite interruption is an explicit uncommitted transaction in the real index, rather than a crash inside the scanner. Complete unpublished stages may remain after death; automatic orphan deletion is not promised. Process-kill checks do not establish power-loss durability.

The ordinary service shares the app's file authority. The isolated-UID control service cannot open those private files. The audio mode described below adds bounded local media processing. GPU execution, background residency, other journal writers and store distribution remain outside this development app.

## Authenticated LAN serving

`ServingActivity` runs the [shared serving library](../mobile-serving/README.md)
in the ordinary app worker, using a separate synthetic journal. It binds only
to the current private Wi-Fi IPv4 address on port 7657. Keep the activity in the
foreground. The app has no unauthenticated HTTP control listener; linked devices
use the production TLS and SPL device door. The APK packages the maintained
`core/payload/solstone` files and copies them into private app storage at startup.

Start it after installing the APK:

```sh
adb shell am start -n app.solstone.development.journal/.ServingActivity
```

Development owner controls use a command/reply file in the app's private files
directory, written through debug USB `run-as`. They call the same production
setup, mark confirmation, finalize, pairing-window and device-removal routes.
Intent extras cannot supply owner requests. Text creation and search must run
from a separate device over the authenticated LAN connection. `index` is an
explicit owner control in this development adapter.

Stopping the runtime closes its listener and carriers. A worker restart reopens the same journal and
identity. If Wi-Fi changes, restart the runtime with the new address. Process
loss leaves a write outcome unknown until an authenticated read reconciles its
stable activity ID. Retrying an existing ID returns the production duplicate
conflict; it does not append another record.

Build the shared development client, then run qualification from a laptop on the
same private network. Supply the phone's Wi-Fi address and the APK hash printed
by `build_apk.py`:

```sh
cargo build --locked --manifest-path tools/mobile-serving/client/Cargo.toml
python3 tools/android-runtime/qualify_serving.py \
  --host PHONE_WIFI_IPV4 --apk-sha256 INSTALLED_APK_SHA256 \
  --client tools/mobile-serving/client/target/debug/ios-serving-client \
  --output /var/tmp/android-serving-qualification
```

Run these commands from the repository root. The default workload lasts at least
20 minutes, with five seconds between completed write, index, query and reconnect
cycles. It also checks consent, invalid credentials, reply loss, duplicate
conflicts, worker and host restart, and revocation of an open carrier. USB carries
private owner controls and evidence; journal writes and searches cross the LAN.
The output includes resource samples and a stopped private-file snapshot checked
by host SQLite. It retains a mode-600 test credential; keep that directory private.
The script force-stops only this development package at completion. It does not
request diagnostic garbage collection.

## Local audio development mode

Build the maintained Parakeet archives with `scripts/mobile_native_build.py
--platform android --components parakeet --work-dir NATIVE_BUILD --cache-dir
DOWNLOAD_CACHE`. Set `PARAKEET_LIB_DIR` to `NATIVE_BUILD/build/parakeet`.
Set `SOLSTONE_FFMPEG_SOURCE_ARCHIVE` to the pinned FFmpeg archive and
`JOURNAL_ONNX_LIBRARY` to the Android `libonnxruntime.so` produced by the
[pinned native preparation](../../docs/ANDROID_CROSS_BUILD.md).
The build records the hashes of these inputs.

Build the APK with `--application-id app.solstone.development.journal.audio`.
This installs alongside the storage development app and the capture app.
After installation, provision the maintained Parakeet model and the checked-in
VAD and speaker models from the repository root:

```sh
python3 tools/android-runtime/provision_audio.py --serial DEVICE_SERIAL \
  --parakeet /path/to/tdt-0.6b-v3-q8_0.gguf
adb shell am start -n app.solstone.development.journal.audio/app.solstone.development.journal.ServingActivity
```

The foreground serving worker automatically discovers accepted audio inputs,
checks their durable ingest receipt and byte identity, runs native decode,
VAD, Parakeet and speaker analysis, and publishes the normal transcript and speaker sidecar.
A paired client reads the result through the existing transcript API. The network contract
remains production multipart `/app/devices/ingest`, protocol version 3, over
paired TLS/SPL. No private command is needed to complete an accepted recording.

this embedded mode admits at most 16 MiB of encoded audio and 120 seconds of
decoded mono 16 kHz samples per input. for multitrack M4A, the decoded sample
budget is shared across tracks before mixing. inputs above either limit receive
a held failure with an explanation in health; their originals are retained.
these holds do not automatically retry on restart. retryable failures are
skipped for the rest of that worker's run; cancel/resume or restart retries them.
cancellation is cooperative between stages and before publication;
it cannot interrupt a native inference already running. serving uses separate
threads, and stopping the worker waits for active native work to return.
these are development-mode bounds, not changes to desktop import support.

`qualify_audio.py` uploads supplied fixtures from a separate LAN client and
checks authenticated transcript retrieval, source bytes, transcript provenance
and SQLite integrity. Raw transcripts remain excluded from search; searchable
summaries require the later Generate pipeline. Run the existing
`qualify_serving.py --package app.solstone.development.journal.audio` separately
for search and serving regressions. The audio qualification
also exercises denied access, retries, lost replies, active cancellation and
worker recovery, silence, malformed media and missing/corrupt model recovery.
Generate the declared fixtures with [audio_fixtures.py](audio_fixtures.py), using
host FFmpeg. The source WAV has a documented expected sentence. The population
includes stereo 44.1 kHz AAC, repeated speech, silence, truncated media and
encoded-size and decoded-duration controls.

```sh
python3 tools/android-runtime/audio_fixtures.py --output /var/tmp/audio-fixtures
python3 tools/android-runtime/qualify_audio.py \
  --host PHONE_WIFI_IPV4 --apk-sha256 INSTALLED_APK_SHA256 \
  --client tools/mobile-serving/client/target/debug/ios-serving-client \
  --fixtures /var/tmp/audio-fixtures --output /var/tmp/android-audio-qualification
python3 tools/android-runtime/run_device.py full \
  --package app.solstone.development.journal.audio
```

The qualification output contains private test credentials and synthetic journal
snapshots. Keep it private. The scripts stop only the selected development app.
