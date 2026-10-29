{

  "description": "Calendar and scheduling interfaces: day/week/month views, agenda lists, event details, event creation forms, booking pages, availability grids, and RSVP/scheduling workflows",
  "output": "json",
  "extraction": "Extract when the visible date range, event detail, availability grid, booking page, or scheduling workflow changes",
  "importance": "high",
  "max_output_tokens": 1792

}

# Calendar Extraction

Extract structured scheduling information from this calendar or scheduling screenshot.

Return JSON matching this shape:

```json
{
  "app": "<the calendar or booking app>",
  "view": "<day|week|month|agenda|event_detail|availability|unknown>",
  "range": "<the visible date range, or null>",
  "events": [
    {
      "title": "<event title, as shown>",
      "start": "<start, as shown>",
      "end": "<end, as shown, or null>",
      "location": "<location, or null>",
      "conferencing": "<conferencing service, or null>",
      "guests": ["<guest name, as shown>"],
      "status": "<response status, or null>",
      "recurrence": "<recurrence, or null>",
      "calendar": "<calendar name, or null>",
      "description": "<event notes, as shown, or null>"
    }
  ],
  "availability": ["<an open slot, as shown>"],
  "notes": "<other visible scheduling context, or null>"
}
```

## Field Notes

- Set `app` to the visible calendar, scheduling, or booking app.
- Set `view` to `day`, `week`, `month`, `agenda`, `event_detail`, `availability`, or `unknown`.
- Use `range` for the visible date range when present; otherwise use null.
- Preserve chronological order in `events`.
- Include event title, start/end, location, conferencing, guests, status, recurrence, calendar name, and description when visible.
- Put booking slots or availability labels in `availability`.
- Use `notes` for host/service names, timezone, booking state, form fields, selected duration, or other visible scheduling context that does not belong to an event.
- Mark unclear text with `[unclear]`.
- Mark cut-off text with `...`.
- Skip unrelated app chrome unless it identifies the calendar account, date range, or scheduling state.

Return ONLY the JSON object.
