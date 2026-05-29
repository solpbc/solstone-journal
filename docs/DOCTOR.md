# solstone Diagnostic Guide

Quick reference for debugging and diagnosing issues. For detailed specifications, see linked documentation.

## Quick Health Check

```bash
# Check if supervisor services are running
pgrep -af "sol:observer|sol:sense|sol:supervisor"

# Check Callosum socket exists
ls -la journal/health/callosum.sock

# Check for stuck agents (should be empty or short-lived)
ls journal/talents/*/*_active.jsonl 2>/dev/null
```

**Healthy state:**
- All three processes running
- `callosum.sock` exists
- `supervisor.status` events show no stale heartbeats
- No `_active.jsonl` files older than a few minutes

---

## Diagnostic Commands

Use the diagnostic command that matches the question:

- `sol doctor` — is the CLI usable on this machine? This universal battery is
  safe on a journal-less or repo-less box.
- `journal doctor` — is this journal host healthy, and what should be fixed?
  This is the health diagnosis view.
- `make preflight` — can a fresh source checkout get ready before `.venv` or
  `uv` exist?
- `sol health` — what live supervisor status is being reported right now?

`sol doctor` runs four checks:

| Check | Severity | Notes |
|-------|----------|-------|
| `python_version` | blocker | Light package-metadata Requires-Python floor; does not require `pyproject.toml`. |
| `sol_importable` | blocker | Verifies the installed/source CLI import path. |
| `local_bin_sol_reachable` | advisory | Checks the expected `~/.local/bin/sol` PATH shape. |
| `stale_alias_symlink` | blocker | Checks only the `sol` wrapper and can migrate recognized legacy aliases. |

`journal doctor` runs the journal-host battery:

| Check | Severity | Notes |
|-------|----------|-------|
| `disk_space` | advisory | Free-space warning. |
| `config_dir_readable` | blocker | Home and service config directory permissions. |
| `journal_dir_writable` | blocker | Journal directory writability when the local journal exists. |
| `service_identity` | blocker | Installed service points at this install. |
| `service_running` | blocker | Service installed/running/crash-loop diagnosis. |
| `journal_sync` | blocker | Concurrent-writer conflict check. |
| `stale_alias_symlink` | blocker | Checks only the `journal` wrapper. |
| `launchd_stale_plist` | advisory | macOS only; Linux skips it. |
| `feature:pdf`, `feature:whisper` | advisory | Optional extras with exact install commands. |

`journal doctor` is role-aware. If there is no local journal directory or no
installed service, folder and service checks emit `skip` (`no local journal` or
`no local journal service`) rather than failing. Invalid service config, service
identity mismatch, crash loops, systemd failed state, and journal-sync conflicts
are blocker failures. An installed service with no supervisor socket is a
warning when the OS unit is not failed. Feature checks are advisory.

`journal setup` step 1 runs `sol doctor --readiness`: the four universal checks
plus `disk_space`, `journal_dir_writable`, `feature:pdf`, and `feature:whisper`.
It does not run runtime service, sync, config-dir, or launchd checks. A blocker
failure still stops setup early; feature advisories stay advisory and include
the exact extra-install command.

`make preflight` runs `scripts/preflight.py`, the stdlib-only source-checkout
readiness battery that is valid before `.venv`/`uv` exist:
`python_version`, `uv_installed`, `venv_consistent`,
`local_bin_sol_reachable`, `disk_space`, and `config_dir_readable`. It shares
probe primitives with doctor through `solstone/think/probe.py`, but its
behavior is unchanged.

---

## Service Architecture

The supervisor (`journal supervisor`) manages these services:

| Service | Command | Purpose | Auto-restart |
|---------|---------|---------|--------------|
| Callosum | (in-process) | Message bus for inter-service events | No |
| Observer | `sol observer` | Screen/audio capture (platform-detected) | Yes |
| Sense | `journal sense` | File detection, processing dispatch | Yes |

Cortex (agent execution) connects to Callosum but runs independently via `journal cortex`.

See [CALLOSUM.md](CALLOSUM.md) for message protocol and [CORTEX.md](CORTEX.md) for agent system.

---

## Log Locations

| What | Where |
|------|-------|
| Current service logs | `journal/health/{service}.log` (symlinks) |
| Daemon stdout/stderr | `journal/health/service.log` (combined, append-only). The managed wrapper exports `PYTHONUNBUFFERED=1` for supervisor runs so stdout/stderr flush in real time and show up in `journal service logs` without a restart. |
| Day's process logs | `journal/{YYYYMMDD}/health/{ref}_{name}.log` |
| Agent execution | `journal/talents/<name>/*.jsonl` |
| Journal task log | `journal/task_log.txt` |

**Symlink structure:** Journal-level symlinks point to current day's logs. Day-level symlinks point to current process instance (by ref).

```bash
# Tail current observer log
tail -f journal/health/observer.log

# Find today's logs
ls -la journal/$(date +%Y%m%d)/health/
```

---

## Health Signals

Health uses a **fail-fast model**: observers exit if they detect problems, and supervisor restarts them. Health is simply whether the observer is running and sending status events.

| Signal | Healthy when | Stale when |
|--------|--------------|------------|
| `hear` | Status received within threshold | No status for 60+ seconds |
| `see` | Status received within threshold | No status for 60+ seconds |

Both signals track the same thing: is the observer alive and communicating? If the observer has capture problems (e.g., screencast files not growing), it exits gracefully and supervisor restarts it.

Staleness threshold: 60 seconds (configurable via `--threshold`).

### Callosum Status Events

Services emit periodic status to Callosum (every 5 seconds when active):

- `observe.status` - Capture state (screencast, audio, activity)
- `cortex.status` - Running agents list
- `supervisor.status` - Service health, stale heartbeats

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

# List today's agents with their prompts
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

### Observer not capturing

```bash
# Check observer log for errors
tail -50 journal/health/observer.log | grep -i error

# Check if observer is emitting status (supervisor.status will show stale_heartbeats)
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

---

## Useful Commands

```bash
# Watch all service logs
tail -f journal/health/*.log

# Count today's agents by status
echo "Completed: $([ -f journal/talents/$(date +%Y%m%d).jsonl ] && wc -l < journal/talents/$(date +%Y%m%d).jsonl || echo 0)"
echo "Running: $(ls journal/talents/*/*_active.jsonl 2>/dev/null | wc -l)"

# Find agents that errored today
jq -r 'select(.status=="error") | .use_id' journal/talents/$(date +%Y%m%d).jsonl 2>/dev/null

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
- After recovery, run `sol indexer` if you need the new screen extracts searchable

---

## See Also

- [logs.md](../talent/journal/references/logs.md) - Journal logs, health files, and event formats
- [CORTEX.md](CORTEX.md) - Agent system, events, configuration
- [CALLOSUM.md](CALLOSUM.md) - Message bus protocol
