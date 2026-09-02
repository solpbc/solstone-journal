# Think

Home-side processing after capture. The process is `journal think`. There is
no Python think package and `make install` does not install one.

## Commands

| Command | Purpose |
|---------|---------|
| `journal think` | Run generators and talents for a day via Cortex |
| `journal supervisor` | Start the local services that feed Convey, Cortex, and background work |
| `journal cortex` | Talent orchestrator |
| `journal talent list` / `journal talent show <name>` | List or inspect talent configs; `--prompt` prints the composed prompt |
| `journal brain status` / `journal brain refresh` | Active-brain status and one bounded check |
| `journal indexer` | Rebuild the search index |
| `solstone call transcripts read` | Read audio and screen transcripts for a day |

```bash
journal think [--day YYYYMMDD] [--segment HHMMSS_LEN] [--stream NAME] [--refresh] [--flush]
journal supervisor [--no-daily] [--no-cortex] [--no-spl] [--no-convey] [--no-schedule]
journal talent list [--schedule daily|segment] [--json]
journal talent show <name> [--prompt] [--day YYYYMMDD] [--segment HHMMSS_LEN] [--full]
```

`--refresh` overwrites existing generator output.

## Architecture

```
journal sense (observe) → chronicle/YYYYMMDD/{stream}/HHMMSS_LEN/
       ↓
journal think
   ├── solstone-core-indexer
   ├── solstone-core-thinking (generators)
   └── solstone-core-cortex → solstone-core __talent-worker
```

Owners:

| Concern | Crate |
|---------|--------|
| `journal think` CLI | `solstone-core-think-cli` |
| generator scheduling and `load` | `solstone-core-thinking` |
| Cortex service | `solstone-core-cortex` |
| talent worker | `solstone-core-talent-runtime` |
| talent configs | `core/payload/solstone/talent/*.md` |
| import | `solstone-core-import` / `solstone-core-import-sources` |
| indexer | `solstone-core-indexer` |
| providers | [PROVIDERS.md](PROVIDERS.md) |
| Cortex events | [CORTEX.md](CORTEX.md) |
| Callosum | [CALLOSUM.md](CALLOSUM.md) |

Agents invoke journal data operations through `solstone call <app> <verb>`. See
[SOLCLI.md](SOLCLI.md).

## Prompt Context Configuration

Generators and talents accept an optional `load` key in frontmatter:

```json
{
  "load": {"transcripts": true, "percepts": false, "talents": {"screen": true}}
}
```

- `false` — do not load this source
- `true` — load if available
- `"required"` — load, and skip generation if nothing is there
- For `agents` only: a dict of agent names, e.g. `{"entities": true, "meetings": "required"}`. `{}` means no agents.

Inline template variables in the `.md` body:

- `$facets` — focused facet context or all facets
- `$activity_context` — activity metadata, segment state, analysis focus

Priority is required on every scheduled prompt. See [CORTEX.md](CORTEX.md).

## Schedules

`config/schedules.json` stores scheduler metadata and named entries. Reserved
keys are `daily_time`, `weekly_day`, and `weekly_time`. Every other top-level
key is a named entry with `cmd` and `every`, plus optional `enabled` and
`max_runtime`. New schedule configurations set daily work to `00:15` and
weekly work to Sunday at `03:15`, keeping both off the hourly boundary. Existing
configurations are not backfilled with those metadata values. Writes go through
`solstone-core-system`.

## Related

- [PROVIDERS.md](PROVIDERS.md) — one active brain, no fallback
- [CORTEX.md](CORTEX.md) — talent spawn and events
- [CALLOSUM.md](CALLOSUM.md) — message bus
- [PROMPT_TEMPLATES.md](PROMPT_TEMPLATES.md) — `$name` and related variables
- [COGITATE.md](COGITATE.md) — cogitate runtime contract
- [GENERATE.md](GENERATE.md) — generate contract
