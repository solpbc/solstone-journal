---
name: solstone
version: 1.0.0
description: >
  Read-only query of your journal (solstone) from any project, plus routing
  guidance for app-specific `solstone call <app>` commands. Look up people and
  relationships, today's events; read transcripts. TRIGGER: solstone, sol, my
  journal, search my memory, what happened, who is, meeting with,
  recall, solstone call routing, solstone call journal/entities/transcripts.
---

# solstone — journal query router

Read-only query interface to your journal. Invoke via Bash:
`solstone call <app> <verb> [flags]`.

Use this skill to search memories, look up people, check today's events, read
transcripts, and choose the right `solstone call` app command from outside the
solstone project context.

## Prerequisites

The `solstone` CLI must be on PATH. Quick check:

```bash
solstone help
```

If this fails, solstone is not installed. Install it from the solstone project:
`journal setup`.

## Capabilities

### recall — search your memory

Search the journal index for anything matching a query.

```bash
solstone call journal search "<query>"
solstone call journal search "<query>" --day 20260327
solstone call journal search "<query>" --facet work
solstone call journal search "<query>" --day-from 20260320 --day-to 20260327
```

Dates use `YYYYMMDD` format. Omit `--day` to search all days. Omit `--facet` to
search all facets.

### who — entities

Look up what the journal knows about a person, company, or project.

```bash
solstone call entities search --query "<query>"
solstone call entities observations "<entity_name>" --facet "<facet>"
solstone call entities network "<entity_name>" --limit 10
solstone call entities history "<entity_name>"
```

### today — what's happening now

Combine these commands to get a picture of the current day:

```bash
solstone call activities list --source anticipated
solstone call journal news "<facet>"
```

### transcript — meeting transcripts

Read what was said during meetings or any recorded time.

```bash
solstone call transcripts scan
solstone call transcripts scan 20260327
solstone call transcripts read
solstone call transcripts read 20260327 --start 140000 --length 120
solstone call transcripts stats
```

`scan` first to see what's available, then `read` with `--start` (HHMMSS,
24-hour format) and `--length` (minutes) to narrow down.

### status — system health

Check if solstone is running and how much data exists.

```bash
solstone call journal storage-summary
solstone call support diagnose
```

## Per-app command map

See [Commands](references/commands.md) for the generated inventory of every
`solstone call <app>` command contributed by app skill fragments, including triggers
and read/write/other polarity. Regenerate it with `make skills`
(`scripts/build_skill_references.py`); do not inline those command tables here.

## Paths

`solstone root` prints the source checkout root for source runs and the installed
`site-packages` root for packaged runs, useful for locating the active install.

## Environment

The `solstone` CLI uses three environment variables that default sensibly:

- `SOL_DAY` — defaults to today (`YYYYMMDD`)
- `SOL_FACET` — defaults to all facets
- `SOL_SEGMENT` — defaults to no segment

External callers should not need to set these. Use explicit flags (`--day`,
`--facet`) when narrowing scope is needed.

## Composing queries

**"Brief me on today"** — events plus active relationships:

```bash
solstone call activities list --source anticipated
solstone call entities search --limit 5
```

**"Prep me for a meeting with X"** — recent transcript mentions:

```bash
solstone call journal search "<name>" --day-from 20260320
```

**"What did I miss yesterday?"** — yesterday's events, transcripts, and news:

```bash
solstone call journal search "" -d 20260326 -a meetings
solstone call transcripts scan 20260326
solstone call journal news "<facet>" --day 20260326
```

## Output format

Most commands output plain text by default. Many support `--json` for structured
output. Prefer plain text for human-readable answers; use `--json` when you need
to process the data further.

## What you cannot do

This is a read-only interface. The journal is the person's private space. You
cannot:

- Create, delete, or modify facets
- Attach or modify entities
- Write news or observations
- Run pipeline operations (think, indexer, transcribe)
- Access internal agent state or orchestration

If a task requires writing to the journal, it must be done from within the
solstone project context using solstone's internal skills.

## Error handling

If `solstone` is not found on PATH or returns an error:

- `"command not found: solstone"` — solstone is not installed. The user needs to run
  `journal setup` in their solstone project.
- `"journal not found"` or empty output — the journal directory doesn't exist or
  has no data yet. solstone may be installed but not initialized.
- Connection errors from `solstone call support` — every support command needs the
  local solstone service reachable. Portal-backed commands (`search`, `article`)
  can additionally fail when the journal host is offline.

Do not retry failed commands. Report the error clearly so the user can
investigate.
