---
name: journal
description: >
  Search the journal, list facets, and explain how the journal is laid out
  on disk — captures, extracts, talent outputs, apps, facets, and the search
  index. Covers host commands such as `journal setup`, `journal doctor`,
  `journal service`, `journal health`, `journal talent`, and `journal
  identity`, plus the `solstone call journal` CLI.
  TRIGGER: journal, journal setup, journal doctor, journal service, journal
  health, journal talent, journal identity, journal layout, search journal,
  find meeting, list facets, show agent output, captures, extracts, talents,
  apps, facet, indexer, activity records, solstone call journal, solstone call journal
  search, solstone call journal facet.
---

# Journal Skill

Operate the local journal host and explore journal layout. Use this skill for
`journal <command>` runtime/setup work and for `solstone call journal <command>`
content queries.

## Overview

A journal is the on-disk record of captures, extracts, facet data, app storage, and talent outputs.

```
┌──────────────────────┐
│ LAYER 3: OUTPUTS     │ talents/<name>.md or .json
├──────────────────────┤
│ LAYER 2: EXTRACTS    │ *.jsonl transcripts, frames, events
├──────────────────────┤
│ LAYER 1: CAPTURES    │ audio/video files
└──────────────────────┘
```

Talent JSON outputs are rendered to text through the formatter registry.

For the full pipeline, see [captures](references/captures.md).

## Host CLI

Use host commands from the journal machine:

```bash
journal setup
journal doctor
journal service status
journal service logs
journal health
journal talent logs
journal identity
```

Boundaries:

- `journal setup` owns first-run setup and repair: config, models, wrappers,
  service units, and the `solstone` + `journal` router skill links.
- `journal doctor` examines only. It can warn when router skills are missing,
  stale, or pointed at the wrong source, but it does not repair them.
- `journal start` starts the supervisor runtime only. It must not initialize
  config, refresh wrappers, repair service units, rebuild references, or fix
  skills.
- `journal service ...` owns service lifecycle: status, start, stop, restart,
  install, uninstall, and logs. `journal up` / `journal down` are aliases for
  service start/stop.
- `journal health` and `journal talent ...` are troubleshooting surfaces for
  supervisor health, logs, pipeline state, and talent run history.
- `journal identity` owns local owner/sol identity operations; use its help
  output before changing identity data.

For app-contributed host command guidance, see
[Commands](references/commands.md).

## Vocabulary

| Term | Definition | Examples |
|------|------------|----------|
| **Day** | 24-hour activity directory | `20250119/` |
| **Segment** | Timestamped capture window | `143022_300/` |
| **Facet** | Project/context scope | `#work`, `#personal` |
| **Entity** | Tracked person/project/tool | People, companies, tools |
| **Activity** | Completed span of one activity type | Meeting, coding session, review |

## Top-Level Layout

| Path | Purpose |
|------|---------|
| `chronicle/` | Daily capture folders |
| `entities/` | Journal-level entity records |
| `facets/` | Facet data: entities, events, news, logs |
| `talents/` | Talent run logs and outputs |
| `solstone/apps/` | App-specific journal storage |
| `imports/` | Imported audio and artifacts |
| `indexer/` | Search index |
| `config/` | Journal configuration and action logs |

For the full table, see [storage](references/storage.md).

## References

- [CLI reference](references/cli.md) — `solstone call journal` commands
- [Configuration](references/config.md) — `journal.json`, providers, retention
- [Facets](references/facets.md) — facet folders, entities, news, activities
- [Captures and Extracts](references/captures.md) — layers, imports, segment layout
- [Logs](references/logs.md) — action logs, token usage, talent logs, health
- [Storage](references/storage.md) — top-level layout, app storage, search index
- [Commands](references/commands.md) — journal-host command guidance contributed by apps (generated)
