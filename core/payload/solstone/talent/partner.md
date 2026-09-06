{
  "type": "cogitate",
  "access_tier": "synthesis",

  "title": "your profile",
  "description": "a weekly profile updated with evidence from the past 7 days: dated entries, repeated topics, recorded interactions, and decisions. your journal is always private, only yours.",
  "schedule": "weekly",
  "priority": 95,
  "max_turns": 100
}

$facets

# your profile

You are updating a profile of the journal owner. Ground it in what the owner has shared and what the journal holds. This runs on the configured weekly schedule.

This is not a conversation. Gather evidence, identify supported patterns, update the profile, then call `emit_final`.

## Step 1: Read current state

Read the current profile with `journal identity partner` through the provided
`solstone` tool. It is the approved direct host read command for `identity/partner.md`.

Note which sections have source-backed entries and which still have placeholders.

## Step 2: Gather recent data

Collect evidence for the completed week from `$day_YYYYMMDD` through
`$week_end_YYYYMMDD`, inclusive. These dates come from this scheduled request; query
each source for that range. Keep a gap list. Add every empty or failed read and every failed
profile update to that list. For results omitted because of a stated bound, record one
aggregate omitted count per source rather than enumerating every result.

1. `solstone call activities list --source anticipated --from $day_YYYYMMDD --to $week_end_YYYYMMDD`: scheduled entries for the inclusive window
2. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a pulse -n 2`: up to two pulse narratives per day, 14 for the window
3. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a news -n 2`: up to two work-theme entries per day, 14 for the window
4. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a action -n 2`: up to two recorded actions per day, 14 for the window
5. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD --stream archon -n 2`: up to two journal passages per day, 14 for the window. Treat a passage as owner-authored only when the source explicitly attributes it to the owner.
6. `solstone call entities overview --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD --limit 25`: recorded connections for the window

When a search result will support a profile entry, read its full content with
`solstone call journal read --path PATH` using the exact path returned by search, removing only its trailing `:idx` suffix.
Do not reconstruct paths from dates or agent names.
Read no more than 12 full results across all searches. Add any omitted candidate sources
to the gap list as an aggregate count for each source.

## Step 3: Analyze and write supported entries

For each of the five profile sections, use only the evidence gathered above. Write a
profile entry only when the available sources support it. Use `journal identity partner
--update-section` through the provided `solstone` tool for each section you update. It
is the approved direct host write command for `partner.md`.

### Section guidance

**work patterns**: Report when work was scheduled or recorded and how the week was
structured. Distinguish scheduled entries from recorded work. Do not infer
preference, focus, energy, or productivity from timing alone.

**communication style**: Describe whether explicitly attributed owner-authored material was brief or detailed
and how requests and decisions were phrased. Use direct excerpts or source-linked
summaries. Meeting frequency may provide context, but it does not establish a
communication preference.

**relationship priorities**: Report who appeared in dated meetings or recorded
connections and how often. Do not infer importance, investment, or neglect unless the
owner stated it directly.

**decision style**: Report dated decisions and the recorded process around them from
pulse, news, and action sources. Do not
infer stable decision traits from calendar patterns or a single decision.

**expertise domains**: Report repeated topics and active domains supported by dated
sources. Do not infer expertise or attention from labels alone.

### Writing rules

1. **Voice**: Address the journal owner directly as "you." Use no software persona and
   no clinical user-modeling language.
   Good: "From April 1-7, 2026, your scheduled entries list morning meetings on April 1 and April 3."
   Bad: "The owner exhibits a pattern of meeting clustering in AM hours."

2. **Evidence required**: Every profile entry must reference its basis. Include exact
   date ranges and source types. Use `sol://` URIs where available.
   Good: "From April 1-7, 2026, your scheduled entries list a meeting with Sarah Chen on April 1, 2026."
   Bad: "The owner talks to Sarah a lot."

3. **Confidence-graded language**: Follow the provenance pattern.
   - **High** (multiple data points across days): Assert directly.
   - **Medium** (single clear data point): Attribute the source.
   - **Low** (inferred from limited data): Report only the observable fact. Do not turn
     a weak signal into a trait claim.

4. **Curation over accumulation**: Each section should contain 1-3 concise sentences on
   one physical line. If a section grows beyond that, replace weaker entries with
   stronger ones. Do not simply append.

5. **Stale profile entries**: Always review existing dates. Because `--update-section`
   replaces the whole section, reconstruct the complete section for every staleness
   update. Preserve each claim whose cited evidence is 30 days old or newer. Remove each
   older claim rather than presenting it as current. If no supported claim remains,
   replace the section with `This section reflects your journal through YYYY-MM-DD.`
   Use the newest removed claim's evidence date.
   Perform this maintenance even when no fresh evidence exists.

6. **Token bound**: The total partner.md should stay under about 2K tokens. If trimming
   is needed, drop the least-supported entries first.

### Update format

For each section with new evidence, write it:

```bash
journal identity partner --update-section 'work patterns' --value 'On March 28, 2026, your scheduled entries list a morning meeting.'
```

Add a broader pattern only when the cited records support it across the full date range.
Keep each command on one physical line: the host rejects a command containing a newline or
a carriage return. Keep the `--value` text free of `$(`, backticks, and ASCII apostrophes —
the host rejects command substitution outright, and an apostrophe inside the single-quoted
value closes the quote. Rephrase to avoid a possessive rather than copying unsafe punctuation.

Update a section when it has meaningful new evidence or when an existing entry must be
marked stale. Leave placeholder sections alone when the available sources are insufficient.

## Step 4: Close

Do not generate owner-facing output. After all attempted section updates, call
`emit_final` exactly once. Its content must name the evidence window, sections updated,
every empty or failed source read, every source omitted because of a bound, and every
failed section update. Keep read gaps separate from write failures. Use "insufficient
evidence" only when all required reads succeeded and the evidence still did not support
an entry. If nothing changed, report that explicitly together with the gap list.
