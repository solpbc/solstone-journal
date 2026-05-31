---
name: health
description: >
  Monitor solstone uptime, troubleshoot capture/processing failures, review
  agent run costs and errors, pipeline health. CLIs: journal health (service),
  journal talent (agent runs), sol call health pipeline (per-day summary).
  TRIGGER: health, status, is it running, service down, errors, agent runs,
  logs, pipeline, journal health, journal talent logs.
---

# Health CLI Skill

Monitor solstone service uptime, troubleshoot failures, and inspect agent runs. Invoke via Bash: `journal health ...`, `journal talent ...`, or `sol call health <command>`.

**Scope note**: Three CLI surfaces live here: `journal health*` (supervisor/service level), `journal talent*` (agent run level), and `sol call health <command>` (app-level pipeline health). They're grouped together because health troubleshooting routinely crosses the three levels.

**Typical workflow**: `journal health` → `journal health logs` → `journal talent logs` → `journal talent log <ID>` for agent-run detail → `sol call health pipeline` for a day-level pipeline summary.

## status

```bash
journal health
```

Show current supervisor status: running services (names, PIDs, uptimes), crashed services, active tasks, queue depths, heartbeat health, and callosum client count.

Connects to `journal/health/callosum.sock` with a 10-second timeout.

Example:

```bash
journal health
```

## logs

```bash
journal health logs [-c N] [-f] [--since TIME] [--service NAME] [--grep PATTERN]
```

View service health logs from today's log files.

- `-c N`: lines per service (default `5`).
- `-f`: follow mode — tail all logs continuously.
- `--since TIME`: filter by time. Accepts relative (`30m`, `2h`, `1d`) or absolute (`4pm`, `16:00`).
- `--service NAME`: filter to one service.
- `--grep PATTERN`: filter lines matching a Python regex.

Behavior notes:

- Reads symlinked logs from `journal/YYYYMMDD/health/*.log`.
- Includes `journal/health/supervisor.log` when no filters are active.
- Log line format: `ISO8601 [service:stream] LEVEL:logger:message`.
- `-f` mode handles symlink target rotation at midnight.

Examples:

```bash
journal health logs
journal health logs -c 20 --service cortex
journal health logs --since 30m --grep "ERROR"
journal health logs -f
```

## agent runs

```bash
journal talent logs [AGENT] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors] [--summary]
```

List recent agent runs.

- `AGENT`: optional agent name filter.
- `-c, --count`: max runs shown (default `20`; `50` when `--daily`).
- `--day YYYYMMDD`: show only runs from a specific day.
- `--daily`: show only daily-scheduled runs.
- `--errors`: show only error runs.
- `--summary`: show grouped aggregation instead of individual lines.

Flags compose with AND logic. For example, `--daily --errors` shows only daily runs that errored.

Output columns: use_id, time, name, status, runtime, cost, events, tools, output_size, model, facet.

Examples:

```bash
journal talent logs
journal talent logs activity -c 10
journal talent logs --daily
journal talent logs --daily --summary
journal talent logs --day 20260228
journal talent logs --daily --errors
```

## agent run detail

```bash
journal talent log <ID> [--json] [--full]
```

Show events for a single agent run.

- `ID`: agent run ID (from `journal talent logs` output).
- `--json`: raw JSONL events.
- `--full`: expanded event detail (no truncation).

Without flags, shows a one-line-per-event timeline: timestamp, event type, detail.

Examples:

```bash
journal talent log 1700000000001
journal talent log 1700000000001 --json
journal talent log 1700000000001 --full
```

## pipeline summary

```bash
sol call health pipeline [--day YYYYMMDD | --yesterday]
```

Summarize think-pipeline health for one day — anomalies, performance metrics, and per-stage outcomes across the day's processing runs. Emits JSON.

- `--day YYYYMMDD`: target day. Defaults to today.
- `--yesterday`: shortcut for yesterday. Mutually exclusive with `--day`.

Use this when you want a day-level view after daily processing completes, rather than a per-run drilldown via `journal talent log`.

Examples:

```bash
sol call health pipeline
sol call health pipeline --yesterday
sol call health pipeline --day 20260115
```

## journal layout

Reference map of key paths. `journal/` is the journal root.

### journal level

| Path | Purpose |
|------|---------|
| `health/` | Service logs: `<service>.log` symlinks, `callosum.sock`, `supervisor.log` |
| `agents/` | Agent run logs: `<name>/<id>.jsonl`, `<name>/<id>_active.jsonl`, `<name>.log` symlink, `<day>.jsonl` day index |
| `config/` | `journal.json`, `convey.json`, `schedules.json`, `actions/YYYYMMDD.jsonl` |
| `facets/<facet>/` | Per-facet data: `facet.json`, `entities/`, `todos/`, `events/`, `news/`, `logs/` |
| `entities/<id>/` | Canonical entity records: `entity.json` |
| `tokens/` | Token usage: `YYYYMMDD.jsonl` per day |
| `indexer/` | Search index: `journal.sqlite` (FTS5) |
| `streams/` | Stream state: `<name>.json` |
| `imports/` | Imported audio and processing artifacts |

### day level (`YYYYMMDD/`)

| Path | Purpose |
|------|---------|
| `<stream>/HHMMSS_LEN/` | Segment folders (captures, extracts, agent outputs) |
| `agents/` | Daily agent outputs: `<name>.md`, `<name>.json` |
| `health/` | Service logs for that day: `<ref>_<service>.log` (symlinked from journal-level `health/`) |
| `stats.json` | Day statistics |

### segment level (`YYYYMMDD/<stream>/HHMMSS_LEN/`)

| Path | Purpose |
|------|---------|
| `audio.*` | Audio captures (`.flac`, `.m4a`, `.ogg`, `.opus`) |
| `<pos>_<connector>_screen.*` | Screen captures (`.webm`, `.mov`, `.mp4`) |
| `audio.jsonl` | Audio transcript extract |
| `<pos>_<connector>_screen.jsonl` | Screen analysis extract |
| `stream.json` | Segment metadata and stream linkage |
| `*.md` | Segment-level agent outputs |

## services

Which services write where:

| Service | Writes to |
|---------|-----------|
| Observer | Audio/video captures in segment folders |
| Sense | Transcripts + screen analysis (JSONL) in segment folders |
| Cortex | Agent JSONL in `agents/<name>/`, outputs in segment/day dirs |
| Indexer | `indexer/journal.sqlite` |
| Supervisor | `health/supervisor.log`, service logs in `YYYYMMDD/health/` |

## Troubleshooting

### `journal health` returns "Connection refused" or times out
The supervisor is not running. Check if `journal supervisor` is active. The owner may need to start solstone with `journal start` or `make dev`.

### Agent run shows "error" status in `journal talent logs`
Run `journal talent log <ID> --full` to see the complete event timeline including the error. Common causes:
- API key issues (rate limits, expired keys)
- Prompt too large (context overflow)
- Network connectivity

### Missing segments or capture gaps
1. Run `journal health` to check observer service status
2. Run `journal health logs --service sense --since 2h` to check for transcription errors
3. Check if the stream is active: `journal streams`

### High agent costs
Run `journal talent logs --summary` for aggregated cost view. Filter by agent: `journal talent logs <agent-name> --summary`.

## Gotchas

- **`journal health` times out at 10 seconds.** If the supervisor is slow or hung, you'll hit the timeout before seeing results. Confirm the supervisor process is alive (`ps` / `journal supervisor` status) before assuming the service is down.
- **Talent log IDs are millisecond timestamps.** `journal talent log 1700000000001` expects the full ID from `journal talent logs`, not a seconds-precision value.
- **`sol call health pipeline` needs today's processing to have run.** Running it at 6am before the daily pipeline has executed will return sparse results for today; use `--yesterday` instead.
