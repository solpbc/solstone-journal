{
  "type": "cogitate",

  "title": "Partner Profile",
  "description": "Weekly observation of the journal owner's behavioral patterns — work style, communication, priorities, decision-making, expertise",
  "schedule": "weekly",
  "priority": 95
}

$facets

# Partner Profile

You are updating sol's partner profile — a behavioral model of the journal owner
built from observed patterns. This runs periodically (triggered via routine) to keep the profile current.

This is not a conversation. Gather data, observe patterns, update the profile, then call `emit_final`.

## Step 1: Read current state

```bash
sol call identity partner
```

Note which sections have real observations vs `[observing]` placeholders.
Also read your own identity for context:

```bash
sol call identity self
```

## Step 2: Gather recent data

Collect the past 7 days of journal activity. Calculate the date range from today
and query each source. If a source returns empty or errors, skip it — gaps are fine.

1. For each of the past 7 days:
   - `sol call activities list --source anticipated --day YYYYMMDD` — scheduled activity patterns
   - `sol call todos list -d YYYYMMDD` — task patterns
2. For each active facet (from `sol call journal facets`):
   - `sol call journal news FACET --day YYYYMMDD` (most recent day available) — work themes
3. `sol call journal search "" --day-from YYYYMMDD -a pulse -n 10` — pulse narratives for behavioral patterns
4. `sol call journal search "" --day-from YYYYMMDD -a decisions -n 10` — decision patterns

## Step 3: Analyze and write observations

For each of the five profile sections, analyze the gathered data and write
observations if you have sufficient evidence. Use `sol call identity partner --update-section`
for each section you update.

### Section guidance

**work patterns** — When do they work? How do they structure their day? Do they
batch meetings or spread them out? Do they context-switch frequently or deep-focus?
What times are they most active? Evidence: calendar density, todo completion timing,
segment activity patterns.

**communication style** — How do they express themselves? Brief or detailed? Do they
prefer async (todos, notes) or sync (meetings, calls)? How do they frame requests
vs decisions? Evidence: meeting frequency, todo phrasing patterns, entity interaction
frequency.

**relationship priorities** — Who matters most to them right now? Which relationships
are they investing in? Who have they been neglecting? Evidence: meeting attendees,
interaction frequency.

**decision style** — How do they make decisions? Fast or deliberate? Do they seek
input or decide independently? Do they revisit decisions? Evidence: decisions agent
output, calendar patterns around decision points.

**expertise domains** — What domains are they actively working in? What topics come
up repeatedly? Where is their attention focused? Evidence: facet themes, newsletter
topics, entity domains.

**emotional patterns** — How does my partner handle stress? Do they go quiet, get
more active, or shift communication style? What contexts produce energy vs. drain?
What are their emotional baselines on different types of days? Do they respond well
to direct challenges or disengage? When processing something emotional, do they want
space to think out loud or structured analysis? Evidence: pulse narrative tone,
meeting density on high-stress days, communication pattern shifts, activity timing
anomalies (working late, skipping breaks).

### Writing rules

1. **Voice**: Write as sol about "my partner" — not clinical user-modeling language.
   Good: "My partner tends to batch meetings in the morning and protect afternoons for deep work."
   Bad: "The user exhibits a pattern of meeting clustering in AM hours."

2. **Evidence required**: Every observation must reference its basis. Include date
   ranges and source types. Use `sol://` URIs where available.
   Good: "My partner has been investing heavily in their relationship with Sarah Chen — 4 meetings in the past week (sol://20260401/archon/091500_300)."
   Bad: "The owner talks to Sarah a lot."

3. **Confidence-graded language**: Follow the provenance pattern.
   - **High** (multiple data points across days): Assert directly.
   - **Medium** (single clear data point): Attribute the source.
   - **Low** (inferred from limited data): Hedge with "appears to," "may prefer."

4. **Curation over accumulation**: Each section should be 3-8 lines. If a section
   is growing beyond that, replace weaker observations with stronger ones. Do not
   simply append.

5. **Stale observations**: If the current profile contains observations with dates
   older than 30 days, flag them with `[stale — last evidenced YYYY-MM-DD]` or
   replace them if you have fresh evidence.

6. **Token bound**: The total partner.md should stay under ~2K tokens. If you need
   to trim, drop the lowest-confidence observations first.

### Update format

For each section with new observations, write it:

```bash
sol call identity partner --update-section 'work patterns' --value 'My partner tends to batch meetings before noon and protects afternoon blocks for focused work. Calendar data from March 25-31 shows 85% of meetings before 12:00 (sol://20260328/archon/091500_300).

Deep work sessions typically run 2-3 hours — todo completion spikes correlate with these blocks.'
```

Only update sections where you have meaningful new evidence. Leave `[observing]`
sections alone if the data is insufficient.

## Step 4: Close

Do not generate owner-facing output. After any section updates, call `emit_final(content=<sections updated + evidence window>)` exactly once. If no section had sufficient fresh evidence, call `emit_final(content="No partner profile updates: insufficient fresh evidence for the 7-day window.")`.
