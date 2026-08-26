# Journal CLI Reference

Use these commands to explore journal content from the terminal.

**Environment defaults**: When `SOL_DAY` is set, commands that take a DAY argument will use it automatically. Same for `SOL_SEGMENT` and `SOL_FACET`.

Common pattern:

```bash
solstone call journal <command> [args...]
```

**Typical workflow**: `search` to find content across all types → `facet` for project detail. For future scheduled items, use `solstone call activities list`.

## search

```bash
solstone call journal search [QUERY] [-n LIMIT] [--offset N] [-d DAY] [--day-from DAY] [--day-to DAY] [-f FACET] [-a AGENT] [--time-bucket BUCKET] [--json]
```

Search the journal index across insights, transcripts, historical event extracts, activity records, and entities.

- `QUERY`: optional text query. Defaults to empty string (`""`), which works as browse mode when filters are provided. Use 2-4 content terms; question words like `what`, `how`, `did`, and `when` usually add noise in this keyword/BM25 index.
- `-n, --limit`: max results (default `10`).
- `--offset`: skip N results (default `0`).
- `-d, --day`: exact day filter (`YYYYMMDD`).
- `--day-from`, `--day-to`: inclusive date-range filters (`YYYYMMDD`).
- `-f, --facet`: facet filter (for example `work`, `personal`).
- `-a, --agent`: agent/content filter (for example `span`, historical `event`, `news`, `entity:detected`).
- `--time-bucket`: time-of-day filter: `morning`, `afternoon`, `evening`, or `night`.
- `--json`: return one structured JSON object with counts, filters, and result items.

Behavior notes:

- FTS5 query syntax:
- Terms are `AND`'d by default.
- Use `OR` for alternatives: `apple OR orange`.
- Use quotes for exact phrases: `"weekly sync"`.
- Use `*` for prefix matching: `migrat*`.
- Zero results means zero. These CLI and agent surfaces do not auto-broaden; broaden by dropping terms, changing to `term1 OR term2`, then adding `*`.
- Use counts with `--facet`, `--agent`, `--day`, and `--time-bucket` to drill down.
- Result ids are `path:idx`; read the underlying file with `solstone call journal read --path <path>` after stripping the `:idx`.
- Use either `--day` or date range flags; do not combine exact day with range filters.

Examples:

```bash
solstone call journal search "incident review" -n 20 -f work
solstone call journal search "standup OR sync" --day-from 20260101 --day-to 20260107
solstone call journal search "" -d 20260115 -a audio
solstone call journal search "weekly sync" --time-bucket morning --json
```

## facet show

```bash
solstone call journal facet show [NAME]
```

Show a comprehensive facet summary.

- `NAME`: facet name (default: `SOL_FACET` env).

Example:

```bash
solstone call journal facet show work
solstone call journal facet show         # uses SOL_FACET
```

## facet create

```bash
solstone call journal facet create <title> [--emoji EMOJI] [--icon ICON] [--color COLOR] [--description DESC] [--consent]
```

Create a new facet directory and initial `facet.json`.

- `title`: display title used for the facet.
- `--emoji`: optional icon emoji (default: `📦`).
- `--icon`: optional Lucide icon name that overrides the emoji-derived interface icon.
- `--color`: optional hex color (default: `#667eea`).
- `--description`: optional description text.
- `--consent`: asserts that the agent has received a direct owner request or explicit owner approval before calling this command. Pass when acting proactively (cogitate, suggestion flows) rather than in direct response to an owner instruction. Adds `"consent": true` to the audit log entry.

Examples:

```bash
solstone call journal facet create "Acme Project"
solstone call journal facet create "Personal" --emoji "🏠" --color "#ff6f61" --description "Life admin"
solstone call journal facet create "Research" --emoji "📚" --icon library
```

## facet update

```bash
solstone call journal facet update <name> [--title T] [--description D] [--emoji E] [--icon ICON] [--color C]
```

Update facet metadata fields.

- `name`: facet identifier.
- `--title`: optional new display title.
- `--description`: optional new description.
- `--emoji`: optional new icon emoji.
- `--icon`: optional Lucide icon name; pass an empty string to clear and use the emoji-derived icon.
- `--color`: optional new hex color.

Example:

```bash
solstone call journal facet update work --description "Client work and planning" --emoji "🛠"
solstone call journal facet update work --icon brain
```

## facet rename

```bash
solstone call journal facet rename <name> <new-name> [--consent]
```

Rename a facet (directory and references in config/chat metadata).

- `name`: current facet identifier.
- `new-name`: new facet identifier.
- `--consent`: asserts that the agent has received explicit owner approval before performing this structural change. Pass when acting proactively rather than in direct response to an owner instruction. Adds `"consent": true` to the audit log entry.

Example:

```bash
solstone call journal facet rename personal personal-life
```

## facet mute

```bash
solstone call journal facet mute <name>
```

Hide a facet from default facet listings.

Example:

```bash
solstone call journal facet mute personal
```

## facet unmute

```bash
solstone call journal facet unmute <name>
```

Show a previously muted facet in default listings again.

Example:

```bash
solstone call journal facet unmute personal
```

## facet delete

```bash
solstone call journal facet delete <name> [--yes] [--consent]
```

Delete a facet directory and all its data.

- `--yes`: skip confirmation prompt.
- `--consent`: asserts that the agent has received explicit owner approval before performing this destructive operation. Agents should always pass both `--consent` and `--yes` when calling delete. Adds `"consent": true` to the audit log entry.

Example:

```bash
solstone call journal facet delete old-facet
solstone call journal facet delete old-facet --yes
```

## facet merge

Facet merging is temporarily unavailable while this command migrates to the native journal surface.

## facets

```bash
solstone call journal facets [--all]
```

List available facets.

- `--all`: include muted facets in the listing.

## agents

```bash
solstone call journal agents [DAY] [-s SEGMENT]
```

List available agent outputs for a day.

- `DAY`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-s, --segment`: optional segment key (default: `SOL_SEGMENT` env).

Without `--segment`, lists daily agent outputs and per-segment outputs. With `--segment`, lists only that segment's outputs.

Example:

```bash
solstone call journal agents 20260115
solstone call journal agents -s 091500_300
```

## read

```bash
solstone call journal read [AGENT] [-d DAY] [-s SEGMENT] [--path PATH] [--max BYTES]
```

Read full content of an agent output or a journal-relative file path.

- `AGENT`: agent name, e.g. `briefing`, `activity`, `screen` (positional argument).
- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-s, --segment`: optional segment key (default: `SOL_SEGMENT` env).
- `--path`: journal-relative file path, such as a search result path after stripping the `:idx` suffix.
- `--max`: max output bytes (default `16384`, `0` for unlimited).

Without `--segment`, reads from the daily agents directory. With `--segment`, reads from that segment's agents directory. With `--path`, pass only the path and do not combine it with `AGENT`, `--day`, or `--segment`.

Examples:

```bash
solstone call journal read briefing -d 20260115
solstone call journal read briefing
solstone call journal read activity -s 091500_300
solstone call journal read --path 20260115/talents/briefing.md
```

## news

```bash
solstone call journal news [NAME] [-d DAY] [-n LIMIT] [--cursor CURSOR] [-w]
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
solstone call journal news work -n 3
solstone call journal news -d 20260115          # uses SOL_FACET
solstone call journal news work --cursor 20260110 -n 5
```

## Talent CLI Boundaries

Cogitate talents have access to all `solstone` commands. The following infrastructure commands must never be called by talents, because they manage services and data pipelines that should only be operated by the supervisor or a human operator:

- `journal supervisor` / `journal start`
- `journal think`
- `solstone import`
- `journal config`
- `journal cortex`
- `journal brain refresh`
- `solstone observe-*`
- `journal sense`
- `journal transcribe` / `journal describe`
- `journal indexer --reset`

Talents should use `solstone call` commands for journal interaction and `journal health` / `journal talent logs` for diagnostics.
