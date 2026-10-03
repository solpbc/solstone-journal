---
name: journal
description: >
  Search the journal, list facets, and explain how the journal is laid out
  on disk — original media, extracts, talent outputs, apps, facets, and the search
  index. Covers host commands such as `solstone journal setup`,
  `solstone journal doctor`, `solstone journal service`, `solstone journal
  health`, and `solstone journal talent` (also run as `journal <command>`), plus
  the `solstone call journal` CLI.
  TRIGGER: journal, solstone journal, journal setup, journal doctor, journal
  service, journal health, journal talent, journal layout, search journal,
  find meeting, list facets, show agent output, original media, captures, extracts, talents,
  apps, facet, indexer, activity records, solstone call journal, solstone call journal
  search, solstone call journal facet.
---

# Journal Skill

Operate the local journal host and explore journal layout. Use this skill for
`solstone journal <command>` runtime/setup work and for
`solstone call journal <command>` content queries.

## Overview

A journal is the on-disk record of original media, extracts, facet data, app storage, and talent outputs.

```
┌──────────────────────┐
│ LAYER 3: OUTPUTS     │ talents/<name>.md or .json
├──────────────────────┤
│ LAYER 2: EXTRACTS    │ *.jsonl transcripts, frames, events
├──────────────────────┤
│ LAYER 1: CAPTURES    │ audio/video files
└──────────────────────┘
```

`CAPTURES`, `EXTRACTS` and `OUTPUTS` are the layer identifiers used in code and
logs. Layer 1 is the original media: the audio and video files as they arrived.

Talent JSON outputs are rendered to text through the formatter registry.

For the full pipeline, see [original media and extracts](references/captures.md).

## Host CLI

Run host commands on the journal's own computer, under `solstone journal`:

```bash
solstone journal setup
solstone journal doctor
solstone journal service status
solstone journal service logs
solstone journal health
solstone journal talent logs
```

`journal <command>` is a shorter name for the same commands, so
`journal doctor` and `solstone journal doctor` do the same thing. These
commands always act on the journal on this computer, never on a paired one.

Boundaries:

- `solstone journal setup` owns first-run setup and repair: config, models,
  wrappers, service units, and the `solstone` + `journal` router skill links.
- `solstone journal doctor` examines only. It can warn when router skills are
  missing, stale, or pointed at the wrong source, but it does not repair them.
- `solstone journal start` starts the supervisor runtime only. It must not
  initialize config, refresh wrappers, repair service units, rebuild
  references, or fix skills.
- `solstone journal service ...` owns service lifecycle: status, start, stop,
  restart, install, uninstall, and logs. `solstone journal up` /
  `solstone journal down` are aliases for service start/stop.
- `solstone journal health` and `solstone journal talent ...` are
  troubleshooting surfaces for supervisor health, logs, pipeline state, and
  talent run history.

Use `solstone journal health --help` for status flags,
`solstone journal health logs --help` for log flags, and
`solstone journal talent --help` to find talent commands.

## Vocabulary

| Term | Definition | Examples |
|------|------------|----------|
| **Day** | 24-hour activity directory | `20250119/` |
| **Segment** | Timestamped window of original media | `143022_300/` |
| **Facet** | Project/context scope | `#work`, `#personal` |
| **Entity** | Tracked person/project/tool | People, companies, tools |
| **Activity** | Completed span of one activity type | Meeting, coding session, review |

## Top-Level Layout

| Path | Purpose |
|------|---------|
| `chronicle/` | Daily folders of original media and everything derived from it |
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
- [Original Media and Extracts](references/captures.md) — layers, imports, segment layout
- [Logs](references/logs.md) — action logs, token usage, talent logs, health
- [Storage](references/storage.md) — top-level layout, app storage, search index
