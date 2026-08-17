{
  "type": "generate",

  "title": "Maintenance Window",
  "description": "Analyzes activity patterns to identify optimal times for scheduled maintenance tasks.",
  "schedule": "daily",
  "priority": 10,
  "output": "json",
  "schema": "daily_schedule.schema.json",
  "hook": {"pre": "daily_schedule", "post": "daily_schedule"},
  "color": "#455a64",
  "thinking_budget": 4096,
  "max_output_tokens": 512,
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

$facets

# Maintenance Window Analysis

You are given a summary of when the owner was active over the past week. Each day lists time windows when activity was recorded. Times not listed represent periods of inactivity.

## Activity Data

$activity_spans

## Task

Find the best time to schedule daily maintenance tasks (backups, syncs, cleanups). The ideal window is:

1. **Consistent** - Inactive at that time across most or all observed days
2. **Long** - A larger gap is better than a smaller one
3. **Reliable** - Prefer times that are consistently inactive over times that vary

## Analysis Steps

1. Map out the 24-hour day and mark which hours show activity on each day
2. Identify hours that are consistently inactive across all days
3. Find the largest contiguous block of inactive hours
4. Select the midpoint of that block as the primary time
5. Select an alternate time in a different inactive block if available

## Output

Return ONLY a JSON object with this exact structure:

```json
{
  "primary": "HH:MM",
  "fallback": "HH:MM"
}
```

- Use 24-hour format (00:00 to 23:59)
- Primary should be in the largest consistent inactive window
- Fallback should be in a different inactive period, or 1 hour offset from primary if only one window exists
- If activity covers the entire day on all days, use "03:00" as primary and "04:00" as fallback

Return ONLY the JSON object, no other text.
