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
- Read one result as last indexed with `solstone call journal read --path PATH --idx IDX --entry-id ENTRY_ID`, copying all three fields from that result. The display `id` is not a file path.
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

Create a new facet directory and initial `facet.json`. The facet's name comes from the title and is returned as `facet`; use that name in later commands. If the name belonged to a facet that was deleted or merged, the new facet gets the next free one, such as `work-2`, and keeps the title you gave.

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
solstone call journal facet rename <name> <new-title> [--consent]
```

Change a facet's title. The facet keeps its name, the one commands and agent permissions use, so everything filed under it stays where it is.

- `name`: the facet's name.
- `new-title`: the title to show for it.
- `--consent`: asserts that the agent has received explicit owner approval before making this change. Pass when acting proactively rather than in direct response to an owner instruction. Adds `"consent": true` to the audit log entry.

Example:

```bash
solstone call journal facet rename personal "Personal life"
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
solstone call journal facet delete <name> --yes [--consent]
```

Delete a facet directory and all its data. Its name can't be given to a new facet afterwards. Run it only after the owner has approved this delete.

- `--yes`: required. Without it the command refuses.
- `--consent`: accepted but changes nothing. Every delete is logged with `"consent": true` in the audit log entry.

Example:

```bash
solstone call journal facet delete old-facet --yes
```

## facet merge

```bash
journal facet merge <source> --into <dest> (--dry-run | --yes) [--consent]
```

Move the contents of facet `<source>` into facet `<dest>`, remove `<source>`, and rebuild the search index. A merge can't be undone. Afterwards `<source>`'s name always means `<dest>`: material that still names it is found under `<dest>`, and no new facet can take that name. Agents limited to `<source>` see nothing until the owner gives them `<dest>`. `.jsonl` logs and entity records that both facets have are combined; where both have the same record id or field, `<dest>`'s is kept. For any other file both facets have, `<dest>` keeps its own copy, `<source>`'s copy is deleted with `<source>`, and the command lists those files. `<source>`'s own facet settings are not carried over. Before asking the owner, run it with `--dry-run` and tell them what it lists; to keep both copies of a listed file, rename one of the two files before merging. This runs with the `journal` command on the computer the journal is on; there is no `solstone call journal` form.

- `--consent`: asserts that the agent has received explicit owner approval before performing this destructive operation. Agents pass it only after the owner has approved this merge. Adds `"consent": true` to the audit log entry.
- `--yes`: required to merge. Without it the command refuses.
- `--dry-run`: changes nothing. It lists the files both facets have that can't be combined, counts per file the records and entity fields that would give way to a different version with the same id or field (for records, the first with each id is kept, reading `<dest>` first; for entity fields, `<dest>`'s value is kept), and says whether `<source>`'s own settings would be dropped. Needs no `--consent`.

Example:

```bash
journal facet merge side-project --into work --dry-run
journal facet merge side-project --into work --consent --yes
```

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

For a search result:

```bash
solstone call journal read --path PATH --idx IDX --entry-id ENTRY_ID
```

Read one search result as last indexed, up to 16,384 bytes. Copy `path`, `idx`,
and `entry_id` from the same result. The index checks all three before returning
content. This reads the indexed entry, not the whole source file. A result may
be one chunk of a longer document. If the entry is no longer indexed, search
again to obtain a current reference. An entry over the limit is refused.

For a whole file or talent output:

```bash
solstone call journal read [AGENT] [-d DAY] [-s SEGMENT] [--path PATH] [--max BYTES]
```

Read full content of an agent output or a journal-relative file path.

- `AGENT`: agent name, e.g. `briefing`, `activity`, `screen` (positional argument).
- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-s, --segment`: optional segment key (default: `SOL_SEGMENT` env).
- `--path`: journal-relative file path. For a search result, also pass `--idx` and `--entry-id` as shown above.
- `--max`: output limit, fixed at `16384` bytes. Other values are refused.

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
- `solstone call speakers repair`
- `solstone call speakers repair-resume`

Talents should use `solstone call` commands for journal interaction and `journal health` / `journal talent logs` for diagnostics.
