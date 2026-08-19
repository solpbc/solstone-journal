---
name: transcripts
description: >
  Browse and read transcript content from audio recordings, screen captures,
  and agent summaries. Check coverage, list segments, read transcripts with
  source filtering, review monthly stats. TRIGGER: transcript, recording,
  audio, what was said, conversation, segment, screen capture, solstone call
  transcripts scan/segments/read/stats.
---

# Transcripts CLI Skill

Inspect transcript availability and content. Invoke via Bash: `solstone call transcripts <command> [args...]`.

**Environment defaults**: When `SOL_DAY` is set, commands that take a DAY argument will use it automatically. `SOL_SEGMENT` and `SOL_STREAM` provide defaults for `--segment` and `--stream` options.

Common pattern:

```bash
solstone call transcripts <command> [args...]
```

**Typical workflow**: `scan` a day for recording windows → `segments` to get segment keys → `read` to retrieve transcript text.

## scan

```bash
solstone call transcripts scan [DAY]
```

Show transcript and percept coverage ranges. Output groups ranges under `Transcripts:` (microphone/system audio) and `Percepts:` (screen activity).

- `DAY`: day in `YYYYMMDD` (default: `SOL_DAY` env).

Use this first to confirm what recording windows exist before running detailed reads.

Example:

```bash
solstone call transcripts scan 20260115
```

## segments

```bash
solstone call transcripts segments [DAY]
```

List recording segments and their source types.

- `DAY`: day in `YYYYMMDD` (default: `SOL_DAY` env).

Behavior notes:

- Segment keys use `HHMMSS_LEN` format (for example `091500_300`).
- Each segment shows start/end time and available types (`audio`, `screen`, or both).

Use this when you want a specific segment key for targeted reads.

Example:

```bash
solstone call transcripts segments 20260115
```

## read

```bash
solstone call transcripts read [DAY] [--start HHMMSS --length MINUTES] [--segment KEY] [--segments KEYS] [--stream NAME] [--full] [--raw] [--transcripts] [--percepts] [--agents]
```

Read transcript content for a day, time range, segment, or span.

- `DAY`: day in `YYYYMMDD` (default: `SOL_DAY` env).

Read modes (mutually exclusive):

1. `--start HHMMSS --length MINUTES`: range mode. Both flags are required together; end time is auto-computed.
2. `--segment KEY`: single segment mode using key from `segments`.
3. `--segments KEY1,KEY2,...`: span mode — comma-separated segment keys, merged in time order.
4. No time flags: whole-day mode.

Source flags:

- `--full`: transcripts + percepts + agents.
- `--raw`: transcripts + percepts (no agents).
- `--transcripts`: transcript content only.
- `--percepts`: screen percepts only.
- `--agents`: agent outputs only.
- Default with no source flags: transcripts + agents (no percepts).
- `--max BYTES`: output is truncated at 16384 bytes by default; use a larger value (for example, `--max 400000`) for full-day or full-transcript reads, or `--max 0` for unlimited output.

`--audio` and `--screen` are hidden aliases for `--transcripts` and `--percepts` respectively. Prefer the primary flags in new code.

Rules:

- Do not combine `--segment`, `--segments`, or `--start/--length` with each other.
- Do not combine `--full` or `--raw` with individual source flags.

Source type meanings:

- **Transcripts**: spoken-word transcripts from microphone/system audio.
- **Percepts**: frame-level screen activity transcription.
- **Agents**: AI-generated summaries and insights.

Examples:

```bash
solstone call transcripts read 20260115
solstone call transcripts read 20260115 --start 090000 --length 30 --raw
solstone call transcripts read 20260115 --segment 091500_300 --full
solstone call transcripts read 20260115 --segments 091500_300,092000_300,092500_300 --transcripts --agents
solstone call transcripts read 20260115 --transcripts
```

## stats

```bash
solstone call transcripts stats MONTH
```

Show daily transcript coverage counts for a month.

- `MONTH`: required month in `YYYYMM`.

Use this to understand recording density and coverage patterns across a month.

Example:

```bash
solstone call transcripts stats 202601
```
