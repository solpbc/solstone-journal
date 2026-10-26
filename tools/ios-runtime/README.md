# journal on a dedicated iOS device

## ordinary owner app

`make build-owner AUDIO_MODELS_DIR=<verified-mobile-models>` builds a separate development app, `app.solstone.journal.owner.dev`, with its own `group.app.solstone.journal.owner.dev` container. `make install-owner` adds installation to the same build. set `DEVICE` and a fresh `BUILD_ROOT`; keep the existing native audio library variables from the local-audio instructions below. the owner build requires bundled speech models. it does not read or update the older runtime development app or its journal.

open **your journal** from its icon. the app opens the maintained journal setup page, where you choose the mark and an existing thinking provider. pair another device through the existing devices page. keep this device plugged in, on your Wi-Fi, with the app in the foreground. home and health show the journal's processing and service status.

the native bar keeps **stop** available if the page fails. stop saves your choice before waiting for processing and serving to drain. opening the app again keeps that choice. **start** opens it again; **restart** first checks that the previous runtime has stopped. leaving the foreground pauses the runtime without treating that interruption as your stop choice. returning to the app resumes only work the journal's existing recovery rules allow. held or uncertain work stays visible in health; restarting does not approve it.

thinking can use an existing bring-your-own provider or confidential processing. the owner build compiles the native library with the `owner-app` feature, so backup and confidential-processing admission accept only this app's own signed resources and refuse the runtime development app's. the existing backup page offers your own storage and sol pbc's encrypted backup. keep your recovery key somewhere separate from this device. the native speech resources are bundled and verified by the existing audio adapter; no USB model provisioning is part of ordinary use.

the web view uses a private custom-scheme adapter to the maintained owner router. it preserves response headers and streams binary originals and live events without an owner HTTP listener. it accepts only its own journal origin and shows other pages, such as a service consent page, in a Safari sheet over the app, so the journal keeps running while you finish there. file uploads from this embedded page are bounded text requests; use a paired device for the mixed-media workload.

build and host checks alone do not qualify physical lifecycle, recovery, upgrades or sustained capacity. this remains a development app, separate from public distribution.

For an isolated owner-flow check, launch the same owner app with `--owner-fixture <name>`. Use lowercase ASCII letters, numbers and internal hyphens, up to 48 bytes; `owner` and `development` are reserved. This selects `Documents/journal-fixture-<name>` in the same signed app-group container and a separate Start/Stop choice. Invalid fixture arguments refuse before opening a journal or reading that choice. With no argument, the existing `journal-owner` and `owner-run-choice.json` stay selected. Supply the fixture argument on every reopen, including cold launches; opening only the icon selects the default journal. Stop and drain one fixture before terminating and relaunching with another; a running process never switches journals. First verify that a newly selected fixture reaches uninitialized setup. Reusing its name intentionally reopens it. `make check-owner-fixture` checks launch selection on the build host.

This development app exercises journal storage, indexing and search with a standard bundled ExtensionFoundation helper. Keep the device plugged in and the host app in the foreground. It requires iOS 26 and a development signing identity. It can serve the production authenticated journal API over a private Wi-Fi interface. It does not integrate with the capture app.

The helper owns persistent files in its private `Documents/journal-<suite>` directory. The host sends typed operations over XPC and receives results. A supplied host path conveys no file authority. The bridge retains an admitted `JournalRoot` and checks its canonical binding before each operation; bound publication uses the production journal IO primitives. Indexing and queries use the production crates. For LAN serving, the foreground host retains the shared [mobile serving runtime](../mobile-serving/README.md). The host and helper independently select the same dedicated app-group journal; the helper performs indexing and export. The maintained router payload is bundled as a signed resource. Exported synthetic results go to the host's Documents directory for independent checking.

## Build and install

On a mac with Xcode 26, XcodeGen, Rust's `aarch64-apple-ios` target and a paired device in Developer Mode:

```sh
cd tools/ios-runtime
make install DEVICE=<device-identifier>
```

`make build` uses the selected iPhoneOS SDK and a generic iOS destination. It generates the Xcode project, builds the native library, generates ExtensionFoundation metadata from the compiler's constant values, then signs and verifies the helper and host. These steps are part of the build. Generated projects and build products are ignored. `BUILD_ROOT` defaults to `/var/tmp/journal-ios-runtime-build`.

The development signing defaults in `project.yml` and `finalize.py` must match the local team, identity and unlocked signing keychain. They use development provisioning only. This app has its own bundle identifier, `app.solstone.journal.runtime.dev`. Installation preserves other apps and their data. Automatic provisioning must authorize `group.app.solstone.journal.runtime.dev` for this host and helper; both refuse serving storage if the signed group container is unavailable.

`make run DEVICE=<device-identifier>` opens the manual controls. `make check` checks native formatting and all-target Clippy on the host. This standalone development crate is separate from the core workspace and release packages.

## Physical qualification

Use a fresh suite name for each run that creates fixtures. These commands execute and verify real native operations, exported SQLite FTS rows, exact fixture content, process identities and foreground heartbeats:

```sh
python3 qualify.py --device <id> --suite acceptance1 --output /tmp/acceptance.log
python3 qualify.py --device <id> --suite controls1 --scenario controls --output /tmp/controls.log
python3 qualify.py --device <id> --suite mixed1 --scenario mixed --count 2048 --duration 1200 --output /tmp/mixed.log
python3 qualify.py --device <id> --suite hostloss1 --scenario hostloss --output /tmp/hostloss.log
python3 qualify.py --device <id> --suite hostloss1 --scenario reopen --output /tmp/reopen.log
```

Acceptance covers create-only refusal, canonical root substitution, worker death at the production pre-publication barrier, committed data after reply loss, connection cancellation, cooperative work cancellation and idle reuse. Fault operations deliberately abort this development helper. A reconnect creates a new process object; the receipts record the actual PID. Connection cancellation does not promise worker termination or roll back committed data.

The controls repeat unpaced production searches with and without a retained read connection. The mixed run rotates updates across 32 memories in a 2,048-memory journal, atomically updates a checkpoint, scans and queries once per cycle for twenty minutes. This is a bounded foreground qualification, not a claim about unattended endurance or background execution. Capture platform resource counters alongside the console when investigating file-backed dirty-memory diagnostics; file sizes, logical write counters and physical flash writes are different measurements.

The host-loss command kills the observed host PID after indexing while the worker is active. The next command opens the same helper-owned journal from a cold host and verifies its files and index. The verifier fails on missing completion, a reported operation error, wrong content or FTS rows, or observed background heartbeats. Re-run the verifier against preserved console output with `--verify-only` and the same arguments. Keep failed runs alongside successful receipts.

All data generated here is synthetic. The fault interface and test-hook dependency belong only to this development app. Do not embed this bridge in a release app.

## Observing durable rebuild progress

Add `--observe-rebuild` to a serving launch to collect read-only evidence from the journal opened by the native runtime. The selected synthetic suite must already contain its `health` and `talents/entities--entity_suggest` directories. The observer checks the native root identity and refuses replaced directories, unsupported file types and malformed records.

Directory notifications trigger bounded reads of `health/work-intent.json` and active entity-suggestion use records. Each changed snapshot includes the original bytes, hashes and a monotonic timestamp. Notifications can coalesce; snapshots do not provide an atomic view across files or a complete event history. A temporary publication race refuses that sample and waits for another notification. These observations do not authorize an interruption or decide recovery eligibility.

The observer allows at most 1 MiB per file, 2 MiB per snapshot, 16 MiB of emitted raw payload and 1,024 notification wakeups within the serving window. Unchanged and refused reads do not consume the emitted-payload allowance. Stop cancels observation and joins descriptor closure before reporting the observer stopped. The default launch leaves observation disabled.

The macOS diagnostic in [RebuildObservationQualification.swift](diagnostics/RebuildObservationQualification.swift) exercises actual Darwin directory events, publication races, replaced identities, rejected file types and concurrent Stop callers. It checks descriptor closure at each Stop return and suppresses later observations. It supplies host controls; physical interruption and recovery require separate evidence.

## Authenticated LAN qualification

The listener and production owner router run on a serial native queue in the foreground host. The standard non-UI helper has a measured 35 MiB dirty-memory limit, below the existing journal mark's 64 MiB Argon2 allocation. Host placement preserves that derivation. The host retains the helper process and private XPC connection for indexing and export. Both derive `Documents/journal-<suite>` from the dedicated signed app-group container; host-supplied paths confer no authority. The earlier storage qualification continues to use the helper's private container. Local owner setup and consent stay inside the app; it opens no unauthenticated loopback port. The host selects its private `en0` IPv4 address and binds only that address. A changed Wi-Fi address requires a restart. Keep the host foreground and the phone plugged in for this development capability.

Build the separate laptop client and run the physical qualification from the mac connected to the phone:

```sh
cargo build --locked --manifest-path tools/mobile-serving/client/Cargo.toml
cd tools/ios-runtime
python3 qualify_serving.py --device <id> --host <phone-wifi-ip> --suite serving1 --client ../mobile-serving/client/target/debug/ios-serving-client --output /tmp/serving1
```

Use a fresh suite and output directory. The default workload runs 120 write/index/query cycles with five seconds between cycles. `--cycles` and `--interval` bound shorter diagnostic runs. The client connects directly to the phone over the existing SPL TLS transport. USB transfers only app-owned control commands and receipts; journal input and search always travel over the LAN.

The harness completes the existing mark-preview, lock and finalize flow, creates a Work facet through Settings, and opens an owner-approved pairing window. It submits synthetic text through the activity-create API, runs the production index scanner through private owner control, then searches through the authenticated API. It checks missing and invalid credentials during a live pairing window, rejects an invalid pairing request, restarts both worker and host, and checks persistent identity and content. Revocation must close an already-authorized carrier and reject a new connection.

Activity creation uses a stable `since_segment`. A repeated create returns a conflict. After an uncertain reply, the harness reads the corresponding record and compares its content before deciding whether to create again. It drops one incomplete request before publication and discards one response after the server begins replying. This qualifies that bounded reconciliation path; it does not promise exactly-once delivery for arbitrary operations.

The host accepts qualification commands from its own `Documents/owner-command.json`, dispatches owner requests in process, sends index/export operations over private XPC, and writes `owner-reply.json`. Each command has an ID, the launched suite name and the `serving` storage selection. The host records consumption before execution and retains the command file through the USB transfer check. A host death can leave an uncertain owner-control reply; it never automatically replays that command. These files are development operator controls inside the app container, with no network route. `serve-stop` closes the listener before the harness snapshots source records and exports SQLite. The verifier independently checks exact source fields, record IDs, FTS population and database integrity, including a deliberately wrong-payload control. Receipts retain source hashes, process IDs, foreground heartbeats, thermal states and resource counters. Credentials remain in a separate mode-0600 file and must stay out of shared receipts.

Incoming TCP listening does not require Local Network privilege on iOS, as described in [Apple TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy). This adapter performs no Bonjour discovery. Outbound connections and discovery have separate permission rules. Physical qualification covers the observed foreground listener lifetime; background serving and unattended endurance remain unqualified.

## Local audio processing

The optional audio build runs local inference on a dedicated thread in the foreground host. It uses the maintained Parakeet C API and the production decoder, VAD, speaker analysis and transcript publication. The [mobile audio adapter](../mobile-audio/README.md) verifies the pinned model before loading it. The app bundles the models; the helper continues to own storage checks and database export.

Stage verified native libraries and models, then build on the mac:

```sh
python3 tools/mobile-audio/prepare.py --retained <ios-native-build-directory> --output /var/tmp/audio-inputs
export PARAKEET_LIB_DIR=/var/tmp/audio-inputs/parakeet
export ORT_LIB_PATH=/var/tmp/audio-inputs/onnx
export ORT_PREFER_DYNAMIC_LINK=false
make -C tools/ios-runtime install AUDIO_MODELS_DIR=/var/tmp/audio-inputs/models DEVICE=<device-identifier>
python3 tools/ios-runtime/qualify_audio.py --device <id> --host <phone-wifi-ip> --suite audio1 --client tools/mobile-serving/client/target/debug/ios-serving-client --output /var/tmp/audio1
```

The [audio qualification](qualify_audio.py) generates synthetic speech on the mac and uploads it through the production protocol-v3 device-ingest route over paired SPL. The phone discovers inputs with accepted custody receipts, decodes and transcribes them locally, and writes normal transcript and speaker outputs. The paired client reads the transcript through the existing segment API. The verifier compares raw-file hashes, persisted and retrieved transcript content, custody records and exported embedding sidecars. Raw transcripts stay out of search; searchable summaries require Generate integration. It records native stage timing, foreground resource counters, thermal state and request latency during processing. Use a fresh suite and output directory; `--smoke` runs only the first speech case.

the embedded processing pass accepts at most 16 MiB of encoded input and 120 seconds of decoded mono audio at 16 kHz. this is a development processing bound, not a qualified duration for every device. over-limit or failed input keeps its original file. these processing limits do not change the ingest format contract. inputs that have reached their retry limit stay held across restarts. model failure leaves the authenticated listener available. cancellation takes effect at native stage boundaries; an active inference call may finish before cancellation is observed. the default build and serving startup leave audio processing disabled.

## Confidential GPU appraisal

The development app uses a separately signed helper for local GPU appraisal. Each request starts a fresh helper, initializes the pinned SDK on its main thread, and uses the profile's pinned CA and RIM resources. The host accepts a result only after confirming that helper's death. Connection cancellation and the helper's independent watchdog terminate native work; the maximum request budget remains 60 seconds. Admission covers the helper, linked native libraries and signed resources before the app registers its transport callback. The production CPU, GPU, nonce and signed-status checks still govern confidential Generate requests.

The opt-in [lifecycle qualifier](qualify_gpu_lifecycle.py) builds a separate diagnostic clone from an explicitly selected clean commit. Its observation code records entry and return at the native callable boundary through a shared memory record. It runs genuine appraisals, invalid proof/nonce/evidence controls, cancellation during an active native call, a 500 ms deadline during an active native call, and successful calls after both interruptions. Each case requires a distinct helper PID and verified death. This diagnostic does not identify which SDK instruction was executing, and its binary differs from the production build.

Use a retained genuine appraisal request containing the profile fingerprint, nonce, GPU evidence, proofs and verification time. Run the qualifier from the committed source tree on the paired mac with the signing keychain unlocked:

```sh
python3 tools/ios-runtime/qualify_gpu_lifecycle.py \
  --commit <exact-commit> --fixture <appraisal-request.json> \
  --work /var/tmp/gpu-lifecycle-source --build /var/tmp/gpu-lifecycle-build \
  --device <device-identifier> --output /var/tmp/gpu-lifecycle-evidence
```

All three output directories must be absent. The tool retains the selected source inputs, app hashes, build and install logs, physical console and final result. Restore the production development app after this diagnostic, then run the normal authenticated service checks separately.
