# solstone Diagnostic Guide

Quick reference for debugging and diagnosing issues. For detailed specifications, see linked documentation.

## Quick Health Check

```bash
# Check the supervisor's services are running (names, pids, uptime, crashes, heartbeat)
solstone journal health

# Check Callosum socket exists
ls -la journal/health/callosum.sock

# Check for stuck agents (should be empty or short-lived)
ls journal/talents/*/*_active.jsonl 2>/dev/null
```

**Healthy state:**
- `solstone journal health` lists `sense` under `Services:` and shows no `Crashed:` section
- `callosum.sock` exists
- `supervisor.status` events show no stale heartbeats
- No `_active.jsonl` files older than a few minutes

---

## Diagnostic Commands

Use the diagnostic command that matches the question:

- `solstone journal doctor` — is this journal host healthy, and what should be fixed?
  This is the health diagnosis view.
- `solstone journal health` — what live supervisor status is being reported right now?

Each doctor check is an independent observation. If a check raises
an ordinary execution exception, the row is reported as `ERROR`, the check result
uses status `fail`, and the aggregate fails independently of that check's
severity. The public result includes the exception type and a truncated message,
not a traceback. Summary `errors` are a subset of `failed`; consumers that want
completed health failures should compute `failed - errors`.

`solstone journal doctor` dispatches to the native `solstone-core doctor` implementation and runs the journal-host battery:

| Check | Severity | Notes |
|-------|----------|-------|
| `disk_space` | advisory | Free-space warning. |
| `config_dir_readable` | blocker | Home and service config directory permissions. |
| `journal_dir_writable` | blocker | Journal directory writability when the local journal exists. |
| `supervisor_conflict` | blocker | macOS only; detects journal.app with the legacy LaunchAgent, or foreign persistent LaunchAgents that relaunch `/Applications/solstone.app`. |
| `service_identity` | blocker | Installed service points at this install. |
| `service_running` | blocker | Service installed/running/crash-loop diagnosis. |
| `journal_sync` | blocker | Concurrent-writer conflict check. |
| `launchd_stale_plist` | advisory | macOS only; remove a positively identified legacy service with `solstone journal service uninstall`, then run the journal app. |
| `default_stt_ready` / `parakeet_cpp_stt_ready` | advisory | Linux Parakeet artifacts, binary loader readiness, model, and running server. A missing `libgomp.so.1` is reported as “OpenMP runtime unavailable” with the distro install command, before the supervisor can collapse it to a generic process exit. |

`solstone journal doctor` is role-aware. If there is no local journal directory or no
installed service, folder and service checks emit `skip` (`no local journal` or
`no local journal service`) rather than failing. Invalid service config, service
identity mismatch, crash loops, systemd failed state, and journal-sync conflicts
are blocker failures. An installed service with no supervisor socket is a
warning when the OS unit is not failed.

On Linux, Parakeet uses the host's GCC OpenMP runtime. Install it with
`sudo apt install libgomp1` on Ubuntu/Debian, `sudo dnf install libgomp` on
Fedora/RHEL, or `sudo pacman -S libgomp` on Arch. The readiness check executes
the pinned CPU binary, so file presence and executable bits alone cannot
produce a false-ready result.

On macOS, `supervisor_conflict` fails when `journal.app` is running while the
legacy `org.solpbc.solstone` LaunchAgent is installed or loaded, or when a
foreign persistent LaunchAgent targets `/Applications/solstone.app`. The proven
legacy remediation is `solstone journal service uninstall`; foreign launcher findings
include one-line `remove foreign launchers targeting /Applications/solstone.app`
commands for the matching plists. In a proven conflict, other diagnoses stay
visible but their action strings point back to resolving the supervisor conflict
first, so the report does not mix service creation, restart, setup, upgrade, or
deletion advice with the conflict fix. If the topology or foreign-launcher scan
is incomplete rather than proven, only service lifecycle actions are withheld
until it can be determined.

`solstone journal setup` step 1 runs `solstone journal doctor --readiness`: `local_bin_solstone_reachable`,
`disk_space`, `journal_dir_writable`, `default_stt_ready`,
`parakeet_cpp_stt_ready`, `speakers_analyze_installation`, and
`vad_runtime_ready`. `local_bin_solstone_reachable` runs on Linux and Windows
only: a mac has no `solstone` on its PATH by design, and the journal app's admin
terminal provides the commands there.
It does not run runtime service, sync, config-dir, or launchd checks. A blocker
failure still stops setup early. An execution error in any readiness check also
stops setup early, even when that check is advisory.

`make preflight` checks a source checkout's build environment: required tools, the pinned Rust
toolchain and platform build libraries. It is read-only and does not require a journal or provider
key. It does not replace `solstone journal doctor`, which checks an installed journal's operational health.
The former Python readiness battery and `solstone/think/probe.py` were retired.

---

## Service Architecture

The supervisor (`solstone journal supervisor`) manages these services:

| Service | Command | Purpose | Auto-restart |
|---------|---------|---------|--------------|
| Callosum | (in-process) | Message bus for inter-service events | No |
| Sense | `solstone journal sense` | File detection, processing dispatch | Yes |

The supervisor normally manages Cortex, which runs completion work and connects to Callosum.
It can also run independently via `solstone journal cortex`.

See [CALLOSUM.md](CALLOSUM.md) for message protocol and [CORTEX.md](CORTEX.md) for the completion lifecycle.

---

## Log Locations

| What | Where |
|------|-------|
| Operational logs | `journal/chronicle/{YYYYMMDD}/health/oplog--*.log`, one file per process run per local day; a run that crosses midnight continues in a new file under the new day. Read them with `solstone journal health logs`. |
| Supervisor's own output | The `service` source in the same directory. When the supervisor runs as an installed service or under the macOS app, it writes its own stdout and stderr there; a manual `solstone journal start` leaves them on the terminal. `solstone journal health logs` includes those lines; `solstone journal service logs` shows the raw tail. |
| Agent execution | `journal/talents/<name>/*.jsonl` |
| Journal task log | `journal/task_log.txt` |

```bash
# Follow every operational log
solstone journal health logs -f

# The supervisor's own lines from the last two days
solstone journal health logs --since 2d --service service -c 200

# Find today's logs
ls -la journal/chronicle/$(date +%Y%m%d)/health/
```

---

## Health Signals

Health uses linked-device evidence: whether the journal is accepting what a paired
client sends, not whether a retired local observer process recently checked in.

`solstone journal doctor` also runs the `client_ingest_health` advisory check. It warns
when the journal has recorded an active client ingest rejection, but never
blocks. An upload whose body stopped arriving before it was complete is not a
rejection: the journal writes a log line for it and leaves the device's record alone. Remediation is to update or restart the client, then
confirm a valid upload clears the active rejection.
`solstone journal doctor` also runs the `client_transport_refusal` advisory check. It
warns when, in the last 7 days, the journal has turned a paired device's requests away because that
device's connection was already carrying as many requests as the journal accepts
at once. It is separate from `client_ingest_health` on purpose: a rejection
happens after a request arrives, a refusal happens instead of one arriving, and
the device sees nothing but the same generic network error either way. Knowing
which of the two occurred is the remediation; a device that keeps provoking it
is worth reporting.

`solstone journal doctor` also runs the `facet_routing` advisory check. Over the newest
two chronicle days it counts active segments whose Sense output filed them under
no facet, and warns above 10%. Activity lists are built from those routes, so an
unrouted segment is missing from them. A journal always keeps one enabled facet
(the supervisor gives a journal with none a `personal` facet at start), so a
warning points at damaged facet declarations or a Sense regression, not an empty
journal.

`solstone journal doctor` reports `capture_health` and `client_delivery_stall` from the journal's record of each assessed device. Both warn when the journal has rejected an upload from a device and accepted nothing from it since; the device's next accepted upload clears it. On a device with more than one source, both also warn when the journal has rejected an upload from one source and accepted nothing from that source since; that source's next accepted upload clears it. Time alone never warns: a device that is asleep or switched off reads as quiet, not as a fault.
Their JSON and JSONL payloads also include registry completeness, delivery state, reach, and any parsed devices that are not yet part of that delivery assessment under `client_delivery`, with the machine reason tokens. Human warnings do not use reach.

### Callosum Status Events

Services emit periodic status to Callosum. Most emit every 5 seconds when active:

- `observe.status` - Capture state (screencast, audio, activity)
- `cortex.status` - Running agents list
- `supervisor.status` - Service health, stale heartbeats

The native `observe.status` event also carries a diagnostics-only health beacon
with the allowlisted fields `name`, `stream_type`, `version`, `uptime`,
`last_successful_sync`, `pending_queue_depth`, `recent_error_count`, and
`last_error_reason`. It is emitted at startup and every 5 seconds, including
when healthy-idle, contains no captured content or file paths, and is distinct
from linked-device uploads and journal-detected ingest rejections.

In journal versions with this check, `solstone journal doctor` reports the
`sense_dispatch` advisory. It reads Sense's latest status update and counts
dispatch errors since any Sense handler last completed successfully. The count
remains during idle periods and stops at 99; the reason is the most recent
error, not a summary of all errors. If Doctor receives no new Sense status
within its 10-second wait ([Doctor's default timeout](../core/crates/solstone-core-doctor/src/context.rs)),
it warns that it could not confirm dispatch health. See [Sense health fields](../core/crates/solstone-core-sense/src/beacon.rs)
and [Sense status updates](../core/crates/solstone-core-sense/src/dispatch.rs).

`stale_heartbeats` in the supervisor's own status does **not** come from `observe.status` — it comes from the supervisor's own peer-heartbeat sync files (`SyncCheckResult.peer_observations`, staleness derived via `sync::native_mtime_seconds`/`HeartbeatClassification`; see `core/crates/solstone-core-system/src/lifecycle/mod.rs`'s `StaleHeartbeatGc`). `observe.status` freshness is a separate, capture-side signal.

See [CALLOSUM.md](CALLOSUM.md) Tract Registry for event schemas.

---

## Reading Agent Files

**Location:** `journal/talents/`

**File states:**
- `{name}/{timestamp}_active.jsonl` - Agent currently running
- `{name}/{timestamp}.jsonl` - Agent completed

**Event sequence** (JSONL, one event per line):

1. `request` - Initial spawn request (prompt, provider, name)
2. `start` - Agent began execution (model info)
3. Historical `tool_start`/`tool_end` - Stored pre-removal tool calls (paired by `call_id`); new Generate runs do not execute tools
4. `thinking` - Model reasoning (if supported)
5. `finish` or `error` - Final result or failure

```bash
# View an agent's final result
jq -r 'select(.event=="finish") | .result' journal/talents/default/1234567890123.jsonl

# List agents in today's journal-day index with their prompts
for id in $(jq -r '.use_id' journal/talents/$(date +%Y%m%d).jsonl 2>/dev/null); do
  f=$(find journal/agents -maxdepth 2 -path "*/${id}.jsonl" -print -quit)
  [ -n "$f" ] || continue
  echo "=== $(basename "$f") ==="
  head -1 "$f" | jq -r '.prompt[:80]'
done
```

See [CORTEX.md](CORTEX.md) for complete event schemas and agent configuration.

---

## Common Issues

### Capture not reaching the journal

```bash
# Check sense log for errors
solstone journal health logs --service sense --grep 'ERROR|error' -c 50

# Check if sense is emitting status via observe.status
# Note: supervisor.status's stale_heartbeats reflects peer heartbeat sync files, not observe.status
```

Causes: DBus issues, screencast permissions, audio device unavailable.

### Agent appears stuck

```bash
# Find active agents
ls -la journal/talents/*/*_active.jsonl

# Check last event in active agent
tail -1 journal/talents/*/*_active.jsonl | jq .
```

Causes: Backend timeout, network issues.

### No Callosum events

```bash
# Verify socket exists
ls -la journal/health/callosum.sock

# Check the background service is running
solstone journal service status
```

Causes: Supervisor not started, socket path permissions.

### Processing backlog

```bash
# Check sense log for queue status
solstone journal health logs --service sense --grep queue -c 10
```

Causes: Slow transcription, describe API rate limits.

### SPL relay / scheduled backup never run on a convey-only setup

**Symptoms:** SPL private link is enabled but the relay never dials; cloud backup shows "enabled" but has never recorded a completed run. This is expected on a **convey-only** setup — a supervisor deliberately started with only the convey component (Cortex and full-think processing excluded from the automatic loop).

`solstone journal spl` and `solstone journal backup run` are both standalone CLI subcommands with no supervisor or IPC dependency — they run correctly when invoked directly, but nothing invokes them on a convey-only setup, because both normally ride the full supervisor's own tick loop, which convey-only skips by design.

```bash
# Confirm both are runnable manually today
solstone journal spl --help
solstone journal backup run
```

**Fix — schedule them yourself, alongside the convey-only service.** The supervisor's own generated launchd plist (`core/crates/solstone-core-service-unit/src/plist.rs`) only launches `solstone journal start <port>`; it does not cover `spl` or `backup run`, so a convey-only setup needs its own separate `launchd` agents. Adjust the `journal` path and journal-path env value to match your install:

```xml
<!-- ~/Library/LaunchAgents/org.solpbc.solstone.spl.plist — keep SPL dialed continuously -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>org.solpbc.solstone.spl</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.local/bin/journal</string>
        <string>spl</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SOLSTONE_JOURNAL</key><string>/Users/YOU/journal</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key>
    <dict><key>SuccessfulExit</key><false/></dict>
    <key>StandardOutPath</key><string>/Users/YOU/journal/health/spl-manual.log</string>
    <key>StandardErrorPath</key><string>/Users/YOU/journal/health/spl-manual.log</string>
</dict>
</plist>
```

```xml
<!-- ~/Library/LaunchAgents/org.solpbc.solstone.backup.plist — run backup on a schedule (StartInterval, not KeepAlive) -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>org.solpbc.solstone.backup</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.local/bin/journal</string>
        <string>backup</string>
        <string>run</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SOLSTONE_JOURNAL</key><string>/Users/YOU/journal</string>
    </dict>
    <key>StartInterval</key><integer>86400</integer>
    <key>StandardOutPath</key><string>/Users/YOU/journal/health/backup-manual.log</string>
    <key>StandardErrorPath</key><string>/Users/YOU/journal/health/backup-manual.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/org.solpbc.solstone.spl.plist
launchctl load ~/Library/LaunchAgents/org.solpbc.solstone.backup.plist
```

On Linux (systemd user, not covered by the example above), the equivalent is a `.timer`/`.service` pair invoking `solstone journal backup run` and a `.service` with `Restart=always` invoking `solstone journal spl`, following the same env-var convention as `solstone-core-service-unit`'s generated unit.

Causes: convey-only is an intentional, documented configuration (not a code defect) that skips the supervisor triggers SPL and backup normally ride. This shape is architecturally generic — any source-checkout running convey-only hits it, not just one machine.

---

## Useful Commands

```bash
# Watch all service logs
solstone journal health logs -f

# Count entries in today's journal-day index by status
echo "Completed: $([ -f journal/talents/$(date +%Y%m%d).jsonl ] && wc -l < journal/talents/$(date +%Y%m%d).jsonl || echo 0)"
echo "Running: $(ls journal/talents/*/*_active.jsonl 2>/dev/null | wc -l)"

# Find agents that errored on today's local execution day
jq -r --arg today "$(date +%Y%m%d)" '
  (.ts | select(type != "boolean") | tonumber?) as $ts
  | select($ts > 0)
  | select((($ts / 1000) | localtime | strftime("%Y%m%d")) == $today)
  | select(.status == "error")
  | .use_id
' journal/talents/????????.jsonl 2>/dev/null

# Check token usage for today
wc -l journal/tokens/$(date +%Y%m%d).jsonl

# Find errors in today's logs
solstone journal health logs --grep 'ERROR|error' -c 200

# Watch Callosum events in real-time
socat - UNIX-CONNECT:journal/health/callosum.sock
```

---

## See Also

- [logs.md](../talent/journal/references/logs.md) - Journal logs, health files, and event formats
- [CORTEX.md](CORTEX.md) - Agent system, events, configuration
- [CALLOSUM.md](CALLOSUM.md) - Message bus protocol
