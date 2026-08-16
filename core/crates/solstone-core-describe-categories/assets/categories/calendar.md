{

  "description": "Calendar and scheduling interfaces: day/week/month views, agenda lists, event details, event creation forms, booking pages, availability grids, and RSVP/scheduling workflows",
  "output": "json",
  "extraction": "Extract when the visible date range, event detail, availability grid, booking page, or scheduling workflow changes",
  "importance": "high",
  "max_output_tokens": 8192

}

# Calendar Extraction

Extract structured scheduling information from this calendar or scheduling screenshot.

Return JSON matching this shape:

```json
{
  "app": "Google Calendar",
  "view": "week",
  "range": "Apr 13 - Apr 19, 2026",
  "events": [
    {
      "title": "Planning review",
      "start": "Tue 10:00 AM",
      "end": "11:00 AM",
      "location": "Conference Room A",
      "conferencing": "Google Meet",
      "guests": ["Alice", "Bob"],
      "status": "accepted",
      "recurrence": null,
      "calendar": "Work",
      "description": "Visible event notes"
    }
  ],
  "availability": ["Tue 2:00 PM", "Wed 10:30 AM"],
  "notes": "Timezone, booking state, host/service, or visible form fields"
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
