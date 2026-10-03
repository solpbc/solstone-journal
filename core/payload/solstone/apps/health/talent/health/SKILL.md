---
name: health
description: >
  Monitor solstone uptime, troubleshoot capture/processing failures, review
  agent runs and errors, pipeline health. CLIs: solstone journal health (service),
  solstone journal talent (agent runs), solstone call health pipeline (per-day summary).
  TRIGGER: health, status, is it running, service down, errors, agent runs,
  logs, pipeline, solstone journal health, solstone journal talent logs.
---

# Health CLI Skill

Monitor solstone service uptime, troubleshoot failures, and inspect agent runs. Invoke via Bash: `solstone journal health ...`, `solstone journal talent ...`, or `solstone call health <command>`.

**Scope note**: Three CLI surfaces live here: `solstone journal health*` (supervisor/service level), `solstone journal talent*` (agent run level), and `solstone call health <command>` (app-level pipeline health). They're grouped together because health troubleshooting routinely crosses the three levels.

**Typical workflow**: `solstone journal health` → `solstone journal health logs` → `solstone journal talent logs` → `solstone journal talent log <ID>` for agent-run detail → `solstone call health pipeline` for a day-level pipeline summary.

## status

```bash
solstone journal health
```

Show current supervisor status: running services (names, PIDs, uptimes), crashed services, active tasks, queue depths, heartbeat health, and callosum client count.

Connects to `journal/health/callosum.sock` with a 10-second timeout.

Example:

```bash
solstone journal health
```

## logs

```bash
solstone journal health logs [-c N] [-f] [--since TIME] [--service NAME] [--grep PATTERN]
```

View operational logs from today, or from `--since` through today.

- `-c N`: number of lines to show, newest last (default `5`).
- `-f`: follow mode — tail all logs continuously. It ignores `-c`, `--since`, `--service` and `--grep`.
- `--since TIME`: filter by time. Accepts relative (`30m`, `2h`, `1d`) or absolute (`4pm`, `16:00`).
- `--service NAME`: filter to one service.
- `--grep PATTERN`: filter lines matching a Python regex.

Behavior notes:

- Reads the logs under `journal/chronicle/YYYYMMDD/health/`, one file per process run per day.
- Process log line format: `ISO8601 [service:stream] message`.
- The supervisor's own output is the `service` source. Its lines are shown as written, without the `[service:stream]` prefix; select them with `--service service`.

Examples:

```bash
solstone journal health logs
solstone journal health logs -c 20 --service cortex
solstone journal health logs --since 30m --grep "ERROR"
solstone journal health logs -f
```

## agent runs

```bash
solstone journal talent logs [AGENT] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors] [--summary]
```

List recent agent runs.

- `AGENT`: optional agent name filter.
- `-c, --count`: max runs shown (default `20`; `50` when `--daily`).
- `--day YYYYMMDD`: show only runs from a specific day.
- `--daily`: show only daily-scheduled runs.
- `--errors`: show only error runs.
- `--summary`: show grouped aggregation instead of individual lines.

Flags compose with AND logic. For example, `--daily --errors` shows only daily runs that errored.

Output columns: use_id, time, name, status, runtime, events, tools, output_size, model, facet.

Examples:

```bash
solstone journal talent logs
solstone journal talent logs activity -c 10
solstone journal talent logs --daily
solstone journal talent logs --daily --summary
solstone journal talent logs --day 20260228
solstone journal talent logs --daily --errors
```

## agent run detail

```bash
solstone journal talent log <ID> [--json] [--full]
```

Show events for a single agent run.

- `ID`: agent run ID (from `solstone journal talent logs` output).
- `--json`: raw JSONL events.
- `--full`: expanded event detail (no truncation).

Without flags, shows a one-line-per-event timeline: timestamp, event type, detail.

Examples:

```bash
solstone journal talent log 1700000000001
solstone journal talent log 1700000000001 --json
solstone journal talent log 1700000000001 --full
```

## pipeline summary

```bash
solstone call health pipeline [--day YYYYMMDD | --yesterday]
```

Summarize think-pipeline health for one day — anomalies, performance metrics, and per-stage outcomes across the day's processing runs. Emits JSON.

- `--day YYYYMMDD`: target day. Defaults to today.
- `--yesterday`: shortcut for yesterday. Mutually exclusive with `--day`.

Use this when you want a day-level view after daily processing completes, rather than a per-run drilldown via `solstone journal talent log`.

Examples:

```bash
solstone call health pipeline
solstone call health pipeline --yesterday
solstone call health pipeline --day 20260115
```

## journal layout

Reference map of key paths. `journal/` is the journal root.

### journal level

| Path | Purpose |
|------|---------|
| `health/` | `callosum.sock` |
| `agents/` | Agent run logs: `<name>/<id>.jsonl`, `<name>/<id>_active.jsonl`, `<name>.log` symlink, `<day>.jsonl` day index |
| `config/` | `journal.json`, `convey.json`, `schedules.json`, `actions/YYYYMMDD.jsonl` |
| `facets/<facet>/` | Per-facet data: `facet.json`, `entities/`, `events/`, `news/`, `logs/` |
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
| `health/` | That day's operational logs: `oplog--*.log`, one file per process run |
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
| Supervisor | its own output and every process log in `chronicle/YYYYMMDD/health/` |

## Troubleshooting

### `solstone journal health` returns "Connection refused" or times out
The supervisor is not running. Check if `solstone journal supervisor` is active. The owner may need to start the service with `solstone journal up` (`make dev` in a dev checkout). ⛔ Do not tell an owner to run `solstone journal start` — it runs the supervisor in the foreground, tied to their terminal, and does not touch the installed service.

### Agent run shows "error" status in `solstone journal talent logs`
Run `solstone journal talent log <ID> --full` to see the complete event timeline including the error. Common causes:
- API key issues (rate limits, expired keys)
- Prompt too large (context overflow)
- Network connectivity

### Missing segments or capture gaps
1. Run `solstone journal health` to check observer service status
2. Run `solstone journal health logs --service sense --since 2h` to check for transcription errors
3. Check if the stream is active: `solstone journal streams`

### Slow or failing agents
Run `solstone journal talent logs --summary` for each agent's completed and failed runs and its runtime range. Filter by agent: `solstone journal talent logs <agent-name> --summary`.

## Gotchas

- **`solstone journal health` times out at 10 seconds.** If the supervisor is slow or hung, you'll hit the timeout before seeing results. Confirm the supervisor process is alive (`ps` / `solstone journal supervisor` status) before assuming the service is down.
- **Talent log IDs are millisecond timestamps.** `solstone journal talent log 1700000000001` expects the full ID from `solstone journal talent logs`, not a seconds-precision value.
- **`solstone call health pipeline` needs today's processing to have run.** Running it at 6am before the daily pipeline has executed will return sparse results for today; use `--yesterday` instead.
