# Journal CLI Reference

Use these commands to explore journal content from the terminal.

**Environment defaults**: When `SOL_DAY` is set, commands that take a DAY argument will use it automatically. Same for `SOL_SEGMENT` and `SOL_FACET`.

Common pattern:

```bash
sol call journal <command> [args...]
```

**Typical workflow**: `search` to find content across all types → `facet` for project detail. For future scheduled items, use `sol call activities list`.

## search

```bash
sol call journal search [QUERY] [-n LIMIT] [--offset N] [-d DAY] [--day-from DAY] [--day-to DAY] [-f FACET] [-a AGENT]
```

Search the journal index across insights, transcripts, historical event extracts, activity records, entities, and todos.

- `QUERY`: optional text query. Defaults to empty string (`""`), which works as browse mode when filters are provided.
- `-n, --limit`: max results (default `10`).
- `--offset`: skip N results (default `0`).
- `-d, --day`: exact day filter (`YYYYMMDD`).
- `--day-from`, `--day-to`: inclusive date-range filters (`YYYYMMDD`).
- `-f, --facet`: facet filter (for example `work`, `personal`).
- `-a, --agent`: agent/content filter (for example `span`, historical `event`, `news`, `entity:detected`).

Behavior notes:

- FTS5 query syntax:
- Terms are `AND`'d by default.
- Use `OR` for alternatives: `apple OR orange`.
- Use quotes for exact phrases: `"weekly sync"`.
- Use `*` for prefix matching: `migrat*`.
- Use either `--day` or date range flags; do not combine exact day with range filters.

Examples:

```bash
sol call journal search "incident review" -n 20 -f work
sol call journal search "standup OR sync" --day-from 20260101 --day-to 20260107
sol call journal search "" -d 20260115 -a audio
```

## facet show

```bash
sol call journal facet show [NAME]
```

Show a comprehensive facet summary.

- `NAME`: facet name (default: `SOL_FACET` env).

Example:

```bash
sol call journal facet show work
sol call journal facet show         # uses SOL_FACET
```

## facet create

```bash
sol call journal facet create <title> [--emoji EMOJI] [--color COLOR] [--description DESC] [--consent]
```

Create a new facet directory and initial `facet.json`.

- `title`: display title used for the facet.
- `--emoji`: optional icon emoji (default: `📦`).
- `--color`: optional hex color (default: `#667eea`).
- `--description`: optional description text.
- `--consent`: asserts that the agent has received a direct owner request or explicit owner approval before calling this command. Pass when acting proactively (cogitate, suggestion flows) rather than in direct response to an owner instruction. Adds `"consent": true` to the audit log entry.

Examples:

```bash
sol call journal facet create "Acme Project"
sol call journal facet create "Personal" --emoji "🏠" --color "#ff6f61" --description "Life admin"
```

## facet update

```bash
sol call journal facet update <name> [--title T] [--description D] [--emoji E] [--color C]
```

Update facet metadata fields.

- `name`: facet identifier.
- `--title`: optional new display title.
- `--description`: optional new description.
- `--emoji`: optional new icon emoji.
- `--color`: optional new hex color.

Example:

```bash
sol call journal facet update work --description "Client work and planning" --emoji "🛠"
```

## facet rename

```bash
sol call journal facet rename <name> <new-name> [--consent]
```

Rename a facet (directory and references in config/chat metadata).

- `name`: current facet identifier.
- `new-name`: new facet identifier.
- `--consent`: asserts that the agent has received explicit owner approval before performing this structural change. Pass when acting proactively rather than in direct response to an owner instruction. Adds `"consent": true` to the audit log entry.

Example:

```bash
sol call journal facet rename personal personal-life
```

## facet mute

```bash
sol call journal facet mute <name>
```

Hide a facet from default facet listings.

Example:

```bash
sol call journal facet mute personal
```

## facet unmute

```bash
sol call journal facet unmute <name>
```

Show a previously muted facet in default listings again.

Example:

```bash
sol call journal facet unmute personal
```

## facet delete

```bash
sol call journal facet delete <name> [--yes] [--consent]
```

Delete a facet directory and all its data.

- `--yes`: skip confirmation prompt.
- `--consent`: asserts that the agent has received explicit owner approval before performing this destructive operation. Agents should always pass both `--consent` and `--yes` when calling delete. Adds `"consent": true` to the audit log entry.

Example:

```bash
sol call journal facet delete old-facet
sol call journal facet delete old-facet --yes
```

## facet merge

Merge all data from a source facet into a destination facet, then permanently delete the source.

```bash
sol call journal facet merge SOURCE --into DEST [--consent]
```

- `SOURCE` — facet to merge from; will be permanently deleted after merge
- `--into DEST` — destination facet to receive all data
- `--consent` — required for agent audit trail when called by an agent

Moves all entities, open todos (not completed or cancelled), non-cancelled calendar events, and news files from SOURCE into DEST. For entity conflicts (entity exists in both): DEST relationship wins, observations are appended without duplicates. For news conflicts (same date file in both): DEST file is preserved, SOURCE file is skipped. Completed/cancelled todos and cancelled events are not moved (they are deleted with the source facet). After all data is moved, SOURCE is permanently deleted and the index is rebuilt.

Prints a summary of what will be moved before performing any mutations.

Example:

```bash
sol call journal facet merge old-project --into archive --consent
```

## facets

```bash
sol call journal facets [--all]
```

List available facets.

- `--all`: include muted facets in the listing.

## agents

```bash
sol call journal agents [DAY] [-s SEGMENT]
```

List available agent outputs for a day.

- `DAY`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-s, --segment`: optional segment key (default: `SOL_SEGMENT` env).

Without `--segment`, lists daily agent outputs and per-segment outputs. With `--segment`, lists only that segment's outputs.

Example:

```bash
sol call journal agents 20260115
sol call journal agents -s 091500_300
```

## read

```bash
sol call journal read AGENT [-d DAY] [-s SEGMENT] [--max BYTES]
```

Read full content of an agent output.

- `AGENT`: agent name, e.g. `briefing`, `activity`, `screen` (positional argument).
- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-s, --segment`: optional segment key (default: `SOL_SEGMENT` env).
- `--max`: max output bytes (default `16384`, `0` for unlimited).

Without `--segment`, reads from the daily agents directory. With `--segment`, reads from that segment's agents directory.

Examples:

```bash
sol call journal read briefing -d 20260115
sol call journal read briefing
sol call journal read activity -s 091500_300
```

## news

```bash
sol call journal news [NAME] [-d DAY] [-n LIMIT] [--cursor CURSOR] [-w]
```

Read or write facet news entries.

- `NAME`: facet name (default: `SOL_FACET` env).
- `-d, --day`: optional specific day (`YYYYMMDD`, default: `SOL_DAY` env).
- `-n, --limit`: max days to return (default `5`).
- `--cursor`: optional pagination cursor (typically a `YYYYMMDD` cutoff for older entries).
- `-w, --write`: write mode — reads markdown from stdin and saves as news for the given day.

Behavior notes:

- Without `--write`: reads and displays existing news entries. Uses `SOL_DAY` to filter to a specific day when set.
- With `--write`: requires `--day` (or `SOL_DAY` env), reads markdown content from stdin, saves to facet news directory.

Examples:

```bash
sol call journal news work -n 3
sol call journal news -d 20260115          # uses SOL_FACET
sol call journal news work --cursor 20260110 -n 5
```

## Talent CLI Boundaries

Cogitate talents have access to all `sol` commands. The following infrastructure commands must never be called by talents, because they manage services and data pipelines that should only be operated by the supervisor or a human operator:

- `journal supervisor` / `journal start`
- `journal think` except heartbeat's targeted `journal think --segment`
- `sol import`
- `journal config`
- `journal cortex`
- `journal providers check`
- `sol callosum`
- `journal observer` / `sol observe-*`
- `journal sense`
- `journal transcribe` / `journal describe`
- `journal indexer --reset`

Talents should use `sol call` commands for journal interaction and `journal health` / `journal talent logs` for diagnostics.
