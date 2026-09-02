# solstone Diagnostic Guide

Quick reference for debugging and diagnosing issues. For detailed specifications, see linked documentation.

## Quick Health Check

```bash
# Check if supervisor services are running
pgrep -af "sol:sense|sol:supervisor"

# Check Callosum socket exists
ls -la journal/health/callosum.sock

# Check for stuck agents (should be empty or short-lived)
ls journal/talents/*/*_active.jsonl 2>/dev/null
```

**Healthy state:**
- Both processes running
- `callosum.sock` exists
- `supervisor.status` events show no stale heartbeats
- No `_active.jsonl` files older than a few minutes

---

## Diagnostic Commands

Use the diagnostic command that matches the question:

- `journal doctor` — is this journal host healthy, and what should be fixed?
  This is the health diagnosis view.
- `journal health` — what live supervisor status is being reported right now?

`journal`-prefixed commands, including `journal doctor` and `journal setup`, require a journal-host install because the `journal` executable ships in the `solstone-journal` distribution, not in the thin `solstone` client.

Each doctor check is an independent observation. If a check raises
an ordinary execution exception, the row is reported as `ERROR`, the check result
uses status `fail`, and the aggregate fails independently of that check's
severity. The public result includes the exception type and a truncated message,
not a traceback. Summary `errors` are a subset of `failed`; consumers that want
completed health failures should compute `failed - errors`.

`journal doctor` dispatches to the native `solstone-core doctor` implementation and runs the journal-host battery without starting Python:

| Check | Severity | Notes |
|-------|----------|-------|
| `disk_space` | advisory | Free-space warning. |
| `config_dir_readable` | blocker | Home and service config directory permissions. |
| `journal_dir_writable` | blocker | Journal directory writability when the local journal exists. |
| `supervisor_conflict` | blocker | macOS only; detects journal.app with the legacy LaunchAgent, or foreign persistent LaunchAgents that relaunch `/Applications/solstone.app`. |
| `service_identity` | blocker | Installed service points at this install. |
| `service_running` | blocker | Service installed/running/crash-loop diagnosis. |
| `journal_sync` | blocker | Concurrent-writer conflict check. |
| `launchd_stale_plist` | advisory | macOS only; stale legacy service plists should be removed with `journal service uninstall`, then repaired with `journal service install` only on a confirmed headless host. |
| `default_stt_ready` / `parakeet_cpp_stt_ready` | advisory | Linux Parakeet artifacts, binary loader readiness, model, and running server. A missing `libgomp.so.1` is reported as “OpenMP runtime unavailable” with the distro install command, before the supervisor can collapse it to a generic process exit. |

`journal doctor` is role-aware. If there is no local journal directory or no
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
legacy remediation is `journal service uninstall`; foreign launcher findings
include one-line `remove foreign launchers targeting /Applications/solstone.app`
commands for the matching plists. In a proven conflict, other diagnoses stay
visible but their action strings point back to resolving the supervisor conflict
first, so the report does not mix service creation, restart, setup, upgrade, or
deletion advice with the conflict fix. If the topology or foreign-launcher scan
is incomplete rather than proven, only service lifecycle actions are withheld
until it can be determined.

`journal setup` step 1 runs `journal doctor --readiness`: `local_bin_solstone_reachable`,
`disk_space`, `journal_dir_writable`, `default_stt_ready`,
`parakeet_cpp_stt_ready`, `speakers_analyze_installation`, and
`vad_runtime_ready`.
It does not run runtime service, sync, config-dir, or launchd checks. A blocker
failure still stops setup early. An execution error in any readiness check also
stops setup early, even when that check is advisory.

⚠ **`make preflight` is gone.** It ran a stdlib-only source-checkout readiness
battery (`python_version`, `uv_installed`, `venv_consistent`,
`local_bin_solstone_reachable`, `disk_space`, `config_dir_readable`) built on
`solstone/think/probe.py`, and both went with the Python reference cut. Nothing
checks source-checkout readiness before `.venv`/`uv` exist today. `journal
doctor` still covers `local_bin_solstone_reachable`, `disk_space` and
`config_dir_readable`, but it needs a working install to run, so it cannot answer
the question preflight existed to answer.

---

## Service Architecture

The supervisor (`journal supervisor`) manages these services:

| Service | Command | Purpose | Auto-restart |
|---------|---------|---------|--------------|
| Callosum | (in-process) | Message bus for inter-service events | No |
| Sense | `journal sense` | File detection, processing dispatch | Yes |

Cortex (agent execution) connects to Callosum but runs independently via `journal cortex`.

See [CALLOSUM.md](CALLOSUM.md) for message protocol and [CORTEX.md](CORTEX.md) for agent system.

---

## Log Locations

| What | Where |
|------|-------|
| Current service logs | `journal/health/{service}.log` (symlinks) |
| Supervisor log (rotated) | `journal/health/supervisor.log` — the supervisor's own RotatingFileHandler sink: 16 MiB active + up to 5 backups (`supervisor.log.1`..`supervisor.log.5`), ≈96 MiB ceiling. Older lines drop on rollover and at startup if a pre-existing file is over cap. |
| Daemon stdout/stderr | `journal/health/service.log` (slow-growing service-manager stdout/stderr sink for startup/status prints; not rotated by the supervisor log handler). Supervisor stdout/stderr is appended to this file by the generated unit (`StandardOutput=append` / `StandardError=append`) so it shows up in `journal service logs` without a restart. |
| Day's process logs | `journal/{YYYYMMDD}/health/{ref}_{name}.log` |
| Agent execution | `journal/talents/<name>/*.jsonl` |
| Journal task log | `journal/task_log.txt` |

**Symlink structure:** Journal-level symlinks point to current day's logs. Day-level symlinks point to current process instance (by ref).

```bash
# Tail current sense log
tail -f journal/health/sense.log

# Find today's logs
ls -la journal/$(date +%Y%m%d)/health/
```

---

## Health Signals

Health uses linked-device evidence: whether a paired client is still adding to
the journal, not whether a retired local observer process recently checked in.

`journal doctor` also runs the `client_ingest_health` advisory check. It warns
when the journal has recorded an active client ingest rejection, but never
blocks. Remediation is to update or restart the client, then confirm a valid
upload clears the active rejection.
`journal doctor` reports `capture_health` and `client_delivery_stall` from whether the solstone app on each assessed device is still adding to the journal.
Their JSON and JSONL payloads also include registry completeness, delivery state, reach, and any parsed devices that are not yet part of that delivery assessment under `client_delivery`. Human warnings use reach only to distinguish an app that is still running but not adding from a device that appears offline and may be asleep; machine reason tokens remain in JSON and JSONL.

| Signal | Healthy when | Stale when |
|--------|--------------|------------|
| `hear` | Status received within threshold | No status for 60+ seconds |
| `see` | Status received within threshold | No status for 60+ seconds |

Both signals track whether a paired client is reaching the journal. If capture
is not reaching the journal, update or restart the client and then send a new
segment.

Staleness threshold: 60 seconds (configurable via `--threshold`).

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

The supervisor checks for `observe.status` event freshness and includes `stale_heartbeats` in its own status.

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
3. `tool_start`/`tool_end` - Tool calls (paired by `call_id`)
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
tail -50 journal/health/sense.log | grep -i error

# Check if sense is emitting status (supervisor.status will show stale_heartbeats)
# Health is derived from solstone.observe.status Callosum events
```

Causes: DBus issues, screencast permissions, audio device unavailable.

### Agent appears stuck

```bash
# Find active agents
ls -la journal/talents/*/*_active.jsonl

# Check last event in active agent
tail -1 journal/talents/*/*_active.jsonl | jq .
```

Causes: Backend timeout, tool hanging, network issues.

### No Callosum events

```bash
# Verify socket exists
ls -la journal/health/callosum.sock

# Check supervisor is running
pgrep -af sol:supervisor
```

Causes: Supervisor not started, socket path permissions.

### Processing backlog

```bash
# Check sense log for queue status
grep -i "queue" journal/health/sense.log | tail -10
```

Causes: Slow transcription, describe API rate limits.

### SPL relay / scheduled backup never run on a convey-only setup

**Symptoms:** SPL private link is enabled but the relay never dials; cloud backup shows "enabled" but has never recorded a completed run. This is expected, not a bug, on a **convey-only** setup — a supervisor deliberately started with only the convey component (Cogitate/Cortex/full-think intentionally excluded from the automatic loop).

`journal spl` and `journal backup run` are both standalone CLI subcommands with no supervisor or IPC dependency — they run correctly when invoked directly, but nothing invokes them on a convey-only setup, because both normally ride the full supervisor's own tick loop, which convey-only skips by design.

```bash
# Confirm both are runnable manually today
journal spl --help
journal backup run
```

**Fix — schedule them yourself, alongside the convey-only service.** The supervisor's own generated launchd plist (`core/crates/solstone-core-service-unit/src/plist.rs`) only launches `journal start <port>`; it does not cover `spl` or `backup run`, so a convey-only setup needs its own separate `launchd` agents. Adjust the `journal` path and journal-path env value to match your install:

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

On Linux (systemd user, not covered by the example above), the equivalent is a `.timer`/`.service` pair invoking `journal backup run` and a `.service` with `Restart=always` invoking `journal spl`, following the same env-var convention as `solstone-core-service-unit`'s generated unit.

Causes: convey-only is an intentional, documented configuration (not a code defect) that skips the supervisor triggers SPL and backup normally ride. This shape is architecturally generic — any source-checkout running convey-only hits it, not just one machine.

---

## Useful Commands

```bash
# Watch all service logs
tail -f journal/health/*.log

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
grep -i error journal/$(date +%Y%m%d)/health/*.log

# Watch Callosum events in real-time
socat - UNIX-CONNECT:journal/health/callosum.sock
```

---

## Recovery Playbooks

### Unfinalized MOV Files (Missing moov Atom)

**Symptoms:** `journal describe` fails with `av.error.InvalidDataError: Invalid data found when processing input`. Sense logs show `describe failed ... exit code 1` and `Segment observed with errors ... ['describe exit 1']`.

**Diagnosis:** The `.mov` file has `ftyp` + `wide` + `mdat` atoms but is missing the `moov` atom. The `mdat` size is 0 (extends-to-EOF). This means the screen recorder (solstone-macos native app) never finalized the file — it wrote video frames but crashed or was interrupted before writing the metadata index.

Known trigger: screen sharing active during solstone-macos native app capture causes AVAssetWriter finalization to be skipped (missing `endSession()` call in `VideoWriter.swift`).

```bash
# Confirm the issue — should report "moov atom not found"
ffprobe -v error journal/YYYYMMDD/STREAM/SEGMENT/center_1_screen.mov

# Inspect atom structure (moov should be present but isn't)
python3 -c "
import struct, os, sys
path = sys.argv[1]
size = os.path.getsize(path)
pos = 0
with open(path, 'rb') as f:
    while pos < size:
        f.seek(pos)
        header = f.read(8)
        if len(header) < 8: break
        atom_size, atom_type = struct.unpack('>I4s', header)
        atom_type = atom_type.decode('ascii', errors='replace')
        flag = '  [extends-to-EOF]' if atom_size == 0 else ''
        if atom_size == 0: atom_size = size - pos
        print(f'  {atom_type:6s} {atom_size:>12,} bytes{flag}')
        pos += atom_size
" /path/to/broken.mov
```

**Recovery:** Extract HEVC parameter sets (VPS/SPS/PPS) from a working sibling file's `hvcC` box, convert the broken file's length-prefixed NALUs to Annex B format, and remux with ffmpeg.

Prerequisites: a good `.mov` from the same stream/session (same codec settings), Python 3, ffmpeg.

```bash
# Step 1: Extract VPS/SPS/PPS from a good reference file
python3 -c "
import struct, os, sys

def find_atom(data, name, offset=0):
    pos = offset
    while pos < len(data) - 8:
        size = struct.unpack('>I', data[pos:pos+4])[0]
        atype = data[pos+4:pos+8]
        if size < 8: break
        if atype == name: return pos, size
        if atype in (b'moov', b'trak', b'mdia', b'minf', b'stbl'):
            result = find_atom(data, name, pos + 8)
            if result: return result
        pos += size
    return None

with open(sys.argv[1], 'rb') as f:
    data = f.read()
pos, size = find_atom(data, b'stsd')
stsd = data[pos:pos+size]
hvcc_off = stsd.find(b'hvcC')
hvcc_size = struct.unpack('>I', stsd[hvcc_off-4:hvcc_off])[0]
cfg = stsd[hvcc_off-4+8:hvcc_off-4+hvcc_size]
offset = 23
with open('/tmp/hevc_params.bin', 'wb') as pf:
    for i in range(cfg[22]):
        num = struct.unpack('>H', cfg[offset+1:offset+3])[0]
        offset += 3
        for j in range(num):
            nalu_len = struct.unpack('>H', cfg[offset:offset+2])[0]
            pf.write(b'\x00\x00\x00\x01')
            pf.write(cfg[offset+2:offset+2+nalu_len])
            offset += 2 + nalu_len
print('Wrote parameter sets to /tmp/hevc_params.bin')
" /path/to/good_reference.mov

# Step 2: Convert broken file to Annex B and remux
python3 -c "
import struct, os, subprocess, sys

src, dst, seg_duration = sys.argv[1], sys.argv[2], int(sys.argv[3])
fsize = os.path.getsize(src)
mdat_offset = 36  # ftyp(20) + wide(8) + mdat_header(8)

with open('/tmp/hevc_params.bin', 'rb') as pf:
    params = pf.read()

annex_b = '/tmp/recovery_raw.h265'
frame_count = 0
with open(src, 'rb') as fin, open(annex_b, 'wb') as fout:
    fout.write(params)
    fin.seek(mdat_offset)
    bytes_read = 0
    mdat_size = fsize - mdat_offset
    while bytes_read < mdat_size - 4:
        lb = fin.read(4)
        if len(lb) < 4: break
        nalu_len = struct.unpack('>I', lb)[0]
        if nalu_len <= 0 or nalu_len > mdat_size - bytes_read: break
        nalu_data = fin.read(nalu_len)
        if len(nalu_data) < nalu_len: break
        nal_type = (nalu_data[0] >> 1) & 0x3f
        if nal_type < 32: frame_count += 1
        fout.write(b'\x00\x00\x00\x01')
        fout.write(nalu_data)
        bytes_read += 4 + nalu_len

fps = f'{frame_count}/{seg_duration}'
print(f'{frame_count} frames, {fps} fps')
subprocess.run(['ffmpeg', '-y', '-v', 'warning', '-r', fps,
    '-f', 'hevc', '-i', annex_b, '-c', 'copy',
    '-movflags', '+faststart', '-tag:v', 'hvc1', dst], check=True)
os.unlink(annex_b)
print(f'Recovered: {dst}')
" /path/to/broken.mov /path/to/recovered.mov DURATION_SECS

# Step 3: Verify recovery
ffprobe -v error -show_streams /path/to/recovered.mov
# Should show codec_name=hevc, correct width/height/duration

# Step 4: Replace original and re-run describe
cp /path/to/recovered.mov /path/to/broken.mov
journal describe /path/to/broken.mov -v
```

**Notes:**
- The segment duration (DURATION_SECS) comes from the segment folder name (`HHMMSS_LEN` — LEN is duration in seconds)
- The reference file must be from the same stream/session so codec parameters match
- PyAV (used by `journal describe`) bundles its own HEVC decoder, so this works even if system ffmpeg lacks one
- After recovery, run `journal indexer` if you need the new screen extracts searchable

---

## See Also

- [logs.md](../talent/journal/references/logs.md) - Journal logs, health files, and event formats
- [CORTEX.md](CORTEX.md) - Agent system, events, configuration
- [CALLOSUM.md](CALLOSUM.md) - Message bus protocol
