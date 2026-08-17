# Prompt Template System

This document describes solstone's template variable system for personalizing prompts used in generators and agents. Templates enable dynamic substitution of owner identity, contextual information, and reusable prompt fragments.

## Overview

Prompts are stored as `.md` files with optional JSON frontmatter for metadata. The prompt content is loaded via `load_prompt()` from `solstone/think/prompts.py`, which uses Python's `string.Template` with `safe_substitute`. This means:

- Variables use `$name` or `${name}` syntax
- Undefined variables are left as-is (no errors)
- Use `$$` to escape a literal dollar sign

The system supports three categories of variables with the following precedence (highest to lowest):

1. **Context variables** - Passed by callers at runtime
2. **Identity variables** - From journal configuration
3. **Template variables** - From reusable template files

## File Format

Prompt files use JSON frontmatter with `{` and `}` as delimiters (braces on their own lines):

```markdown
{
  "title": "Activity Synthesis",
  "color": "#00bcd4",
  "schedule": "segment"
}

$segment_preamble

# Segment Activity Synthesis

Your prompt content here...
```

Files without metadata can omit the frontmatter entirely - just write the prompt content directly.

## Variable Categories

### Identity Variables

Identity variables come from the `identity` block in `config/journal.json`. These are available in all prompts automatically.

**Common variables:**
- `$name` - Full name
- `$preferred` - Preferred name or nickname
- `$bio` - Self-description
- `$timezone` - IANA timezone identifier

**Pronoun variables** (flattened from nested structure):
- `$pronouns_subject` - e.g., "he", "she", "they"
- `$pronouns_object` - e.g., "him", "her", "them"
- `$pronouns_possessive` - e.g., "his", "her", "their"
- `$pronouns_reflexive` - e.g., "himself", "herself", "themselves"

**Uppercase-first versions** are automatically generated for all identity variables:
- `$Name`, `$Preferred`, `$Bio`
- `$Pronouns_subject`, `$Pronouns_possessive`, etc.

The flattening logic converts nested objects using underscore separators. For example, `identity.pronouns.subject` becomes `$pronouns_subject`.

**References:**
- Identity configuration: [config.md](../talent/journal/references/config.md) (identity section)
- Flattening implementation: `solstone/think/prompts.py` → `_flatten_identity_to_template_vars()`

### Template Variables

Template variables come from `.md` files in the `core/payload/solstone/think/templates/` directory. Each file's stem becomes a variable name containing its contents.

**Current templates:**
- `$daily_preamble` - Preamble for full-day output analysis
- `$segment_preamble` - Preamble for single-segment analysis
- `$activity_preamble` - Preamble for activity-level analysis (uses `$activity_*` context variables)

Templates can themselves use identity and context variables, enabling composable prompt construction. For example, `daily_preamble.md` uses `$preferred` and `$day`.

**Pattern:** To add a new template variable, create `core/payload/solstone/think/templates/mytemplate.md` and it becomes available as `$mytemplate` in all prompts.

**Reference:** `core/payload/solstone/think/templates/` directory

### Context Variables

Context variables are passed at runtime by the code calling `load_prompt()`. These are use-case specific and not globally available.

**Common generator context:**
- `$day` - Human-readable date (e.g., "Friday, January 24, 2026")
- `$day_YYYYMMDD` - Day in YYYYMMDD format (e.g., "20260124")
- `$facet` - Focused facet name when the prompt is dispatched per-facet (bare name, e.g. `work`)
- `$activity_md_dir` - Directory path containing per-activity narrative `.md` outputs for the focused facet/day, with trailing slash
- `$now` - Current date and time with timezone (e.g., "Monday, February 3, 2025 at 10:30 AM PST")
- `$segment` - Segment key (e.g., "143022_300")
- `$segment_start` - Formatted start time (e.g., "2:30 PM")
- `$segment_end` - Formatted end time (e.g., "2:35 PM")

**Activity context** (available for `schedule: "activity"` agents):
- `$activity_id` - Activity record ID (e.g., "coding_095809_303")
- `$activity_type` - Activity type (e.g., "coding", "meeting")
- `$activity_description` - Description of the activity
- `$activity_level` - Average engagement level (0-1)
- `$activity_entities` - Comma-separated active entities
- `$activity_segments` - Comma-separated segment keys
- `$activity_duration` - Estimated duration in minutes

Context variables also get automatic uppercase-first versions (`$Day`, `$Day_yyyymmdd`, etc.).

**References:**
- Generator context building: `solstone/think/generate.py` (search for `prompt_context`)

## Usage Patterns

### For Generators

Generator prompts typically compose a shared preamble with agent-specific instructions:

```markdown
{
  "title": "My Generator",
  "color": "#4caf50",
  "schedule": "segment"
}

$segment_preamble

# Segment Activity Synthesis

Your specific instructions here...
```

The `$segment_preamble` or `$daily_preamble` template provides standardized context about what's being analyzed, while the rest of the prompt defines the specific analysis task.

**Optional model configuration:** Add `max_output_tokens` (response length limit) and `thinking_budget` (model thinking token budget) to override provider defaults.

**Reference:** `core/payload/solstone/talent/*.md` for examples (files with `schedule` field but no `tools` field)

### For Agents

Agent prompts are `.md` files with configuration in frontmatter:

```markdown
{
  "title": "My Agent",
  "tools": "journal"
}

You are a helpful assistant...
```

**Optional model configuration:** Add `max_output_tokens` (response length limit) and `thinking_budget` (model thinking token budget) to tune the request sent through the active brain. Note: OpenAI uses fixed reasoning and ignores `thinking_budget`.

**Reference:** `solstone/think/talent.py` → `get_talent()` for agent configuration loading

### The load_prompt() Function

```python
load_prompt(
    name: str,                      # Prompt filename (without .md)
    base_dir: Path | None = None,   # Directory containing prompt
    context: dict | None = None,    # Runtime context variables
) -> PromptContent
```

Returns a `PromptContent` named tuple with `text` (substituted content), `path` (source file), and `metadata` (frontmatter dict).

**Reference:** `solstone/think/prompts.py` → `load_prompt()`

## Adding New Variables

### Identity Variables

Edit `config/journal.json` to add or modify identity fields. Nested objects are automatically flattened with underscore separators.

### Template Variables

Create a new `.md` file in `core/payload/solstone/think/templates/`. The filename stem becomes the variable name.

### Context Variables

Pass via the `context` parameter when calling `load_prompt()`:

```python
load_prompt("myprompt", context={"custom_var": "value"})
```

## Reference Index

| Category | Authoritative Source |
|----------|---------------------|
| Identity config schema | [config.md](../talent/journal/references/config.md) (identity section) |
| Identity flattening | `solstone/think/prompts.py` (`_flatten_identity_to_template_vars`) |
| Template loading | `solstone/think/prompts.py` (`_load_templates`) |
| Core load function | `solstone/think/prompts.py` (`load_prompt`) |
| Template files | `core/payload/solstone/think/templates/*.md` |
| Test coverage | `tests/test_template_substitution.py` |
| Generator prompts | `core/payload/solstone/talent/*.md` (files with `schedule` field but no `tools`) |
| Agent prompts | `core/payload/solstone/talent/*.md` (files with `tools` field) |
