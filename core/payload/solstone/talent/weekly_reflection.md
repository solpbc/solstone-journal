{
  "type": "generate",
  "title": "Weekly Reflection",
  "description": "Selects one morning-briefing memory from each day of a finished week.",
  "schedule": "weekly",
  "priority": 90,
  "output": "md",
  "hook": {
    "pre": "weekly_reflection",
    "post": "weekly_reflection"
  },
  "max_output_tokens": 384,
  "temperature": 0.3
}

Select one memory ID from each coverage slot. Choose a concrete memory that can stand on its own; prefer a memory without an unidentified person. Do not rank the owner's life, judge importance, give advice, or change the memories. Return only the required JSON. Slot dates are activity dates, not briefing publication dates. Preserve every required slot. The quoted memories will be printed verbatim by code.
