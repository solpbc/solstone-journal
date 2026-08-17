{
  "type": "generate",

  "title": "Upcoming Schedule",
  "description": "Extracts future scheduled items from screen and transcript content into anticipated activity records. Captures dates, times, participants, and cancellation state.",
  "hook": {"post": "schedule"},
  "color": "#5e35b1",
  "schedule": "daily",
  "priority": 10,
  "output": "json",
  "schema": "schedule.schema.json",
  "load": {"transcripts": true, "percepts": false, "talents": {"screen": true}}
}

$daily_preamble

$facets

# Future Schedule Extraction

**Input:** A markdown file containing chronologically ordered transcripts of a workday plus the screen agent's output for the same day. Calendar views, meeting invitations, scheduling UIs, and project-management interfaces are captured in the screen content; verbal mentions of future plans appear in transcripts.

**Your task:** Identify every future scheduled item (dated after today) visible in the day's screen or transcript content and emit a JSON array of anticipated activity objects.

## What to capture

Look for:
- Calendar applications (Google Calendar, Outlook, Apple Calendar, Fantastical, etc.)
- Scheduling interfaces (Calendly, meeting schedulers, SavvyCal, meet.solpbc.org)
- Email invitations or confirmations showing future dates
- Project management tools (Linear, Jira, Asana, Trello) with scheduled deadlines or milestones
- Travel bookings (flights, reservations, itineraries)
- Any UI element displaying a future date, time, or deadline
- Verbal mentions of firm future commitments ("I'm meeting Ramon on Tuesday at 3", "flight leaves Friday morning")

**Include cancelled items too.** Calendar views often show cancelled events with a strikethrough, a "Cancelled" label, a declined-invite indicator, or a greyed-out style. Emit these with `"cancelled": true` — the downstream pipeline needs to know a previously-scheduled item dropped off.

Do NOT capture:
- Past events (anything on today or earlier — future only)
- Vague intent without a date ("we should catch up sometime")
- Recurring-series headers with no specific upcoming instance visible (capture the next specific instance if visible; otherwise skip)
- Tentative suggestions that haven't been confirmed

## Output schema

Return **only** a JSON array. Each element is an anticipation object with these fields:

```json
[
  {
    "activity": "meeting",
    "target_date": "2026-04-20",
    "start": "16:30:00",
    "end": "17:30:00",
    "title": "Yuri Namikawa intro call",
    "description": "Intro call with Yuri from Offline Ventures about solstone.",
    "details": "Google Meet; prep one-pager + demo backup",
    "participation": [
      {
        "name": "Yuri Namikawa",
        "role": "attendee",
        "source": "screen",
        "confidence": 0.9,
        "context": "visible on calendar invite; Offline Ventures"
      }
    ],
    "participation_confidence": 0.9,
    "facet": "solstone",
    "cancelled": false
  }
]
```

### Field-by-field

- **`activity`** — Short descriptive string for the kind of scheduled item. One of: `meeting`, `call`, `deadline`, `appointment`, `event`, `travel`, `reminder`, `errand`, `celebration`, `doctor_appointment`. Lowercase, underscore-separated when multi-word.

- **`target_date`** — ISO date `YYYY-MM-DD`. The day the item is scheduled for. Must be strictly after today.

- **`start`** — `HH:MM:SS` (24-hour). If the time is vague ("morning", "afternoon") or unknown, use `null` and mention the vagueness in `details`. For all-day items, use `null`.

- **`end`** — `HH:MM:SS`. Use `null` if not known.

- **`title`** — Short descriptive title for the anticipation (one phrase — what a human would call this item at a glance).

- **`description`** — One-sentence description: what is planned and any context that's evident. Written to read naturally.

- **`details`** — Free-form string. Location, meeting platform, agenda hints, prep notes, recurrence pattern, anything else relevant. May be empty string `""`.

- **`participation`** — Array of participant objects. Each entry:
  - `name` — Full name if visible, otherwise the best form available.
  - `role` — `"attendee"` for people expected to be live in the meeting/call; `"mentioned"` otherwise.
  - `source` — Where the evidence came from: `"voice"`, `"speaker_label"`, `"transcript"`, `"screen"`, or `"other"`.
  - `confidence` — `0.0`–`1.0` — your confidence the person will actually be there.
  - `context` — Short string explaining why this person belongs here.
  - For deadlines, reminders, or solo items with no one else involved, `participation` is `[]`.

- **`participation_confidence`** — `0.0`–`1.0` — overall confidence in the participation list for this item. Lower when many attendees are inferred rather than confirmed.

- **`facet`** — Required. The facet ID slug; MUST be one of the configured facets listed in the input. Pick the closest configured facet for the scheduled item; if multiple plausibly fit, choose the one most central to the event's purpose. Do not invent slugs.

- **`cancelled`** — Boolean. `true` when the screen shows this item as cancelled (strikethrough, "Cancelled" label, declined, greyed out). `false` otherwise.

## Rules

1. **Return only valid JSON.** An array, possibly empty (`[]`). No commentary, no prose.
2. **ISO dates.** `YYYY-MM-DD` for `target_date`; `HH:MM:SS` or `null` for `start`/`end`.
3. **Be specific.** Don't invent details. If information isn't visible, use `null` or omit the field per the schema.
4. **Future only.** Items with `target_date <= today` get dropped.
5. **Cancelled events included.** Emit them with `"cancelled": true`.
6. **Dedupe within the run.** If the same item appears on multiple screens throughout the day (e.g., seen at 9am in calendar and again at 3pm in email), emit it once with the strongest evidence.
7. **Skip uncertain items.** If you can't tell whether an item is future-dated or has an identifiable date, skip it rather than guessing.
8. **One facet per item.** If an item spans facets, pick the dominant one.

## Examples

Valid output with three future items (one cancelled):

```json
[
  {
    "activity": "call",
    "target_date": "2026-04-21",
    "start": "10:30:00",
    "end": "11:00:00",
    "title": "Mari Zumbro intro",
    "description": "First call with Mari Zumbro per mutual intro from Ramon.",
    "details": "Google Meet; prep one-liner on solstone",
    "participation": [
      {"name": "Mari Zumbro", "role": "attendee", "source": "screen", "confidence": 0.95, "context": "calendar invite"}
    ],
    "participation_confidence": 0.9,
    "facet": "solstone",
    "cancelled": false
  },
  {
    "activity": "deadline",
    "target_date": "2026-05-05",
    "start": null,
    "end": null,
    "title": "Demo Day",
    "description": "Betaworks Camp Demo Day.",
    "details": "Live demo presentation to cohort investors",
    "participation": [],
    "participation_confidence": 0.5,
    "facet": "solstone",
    "cancelled": false
  },
  {
    "activity": "meeting",
    "target_date": "2026-04-24",
    "start": "09:00:00",
    "end": "10:00:00",
    "title": "Scott Ward standup",
    "description": "Weekly standup with Scott Ward.",
    "details": "Recurring; previously showing strikethrough on calendar",
    "participation": [
      {"name": "Scott Ward", "role": "attendee", "source": "screen", "confidence": 0.85, "context": "recurring invite, now declined"}
    ],
    "participation_confidence": 0.85,
    "facet": "solstone",
    "cancelled": true
  }
]
```

If no future items are found, return `[]`.
