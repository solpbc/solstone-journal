{

  "description": "Video calls/conferencing (Zoom, Meet, Teams, Webex, etc.)",
  "output": "json",
  "extraction": "Extract when shared content type changes (screen share vs participant grid) or meeting platform differs",
  "importance": "high"

}

# Meeting State Analysis

You are analyzing a screenshot to capture detailed meeting information. You receive a full screenshot showing an active video call or meeting.

Respond with JSON describing the meeting state:

```json
{
  "platform": "<meeting platform: zoom|meet|teams|slack|discord|webex|other>",
  "participants": [
    {
      "name": "<participant name if visible>",
      "status": "<speaking|muted|active|presenting|unknown>",
      "video": <true|false>,
      "box_2d": [ymin, xmin, ymax, xmax]
    }
  ],
  "screen_share": {
    "box_2d": [ymin, xmin, ymax, xmax],
    "presenter": "<name of person presenting, or null>",
    "description": "<brief description of shared content>",
    "formatted_text": "<full text extraction in markdown>"
  }
}
```

## Field Notes

- **platform**: Identify the video conferencing platform being used
- **participants**: List all visible participants with their current state
  - **name**: Extract name from display label if visible, otherwise use "Unknown"
  - **status**: Determine from visual cues (speaking indicators, muted icons, etc.)
  - **video**: Is their video feed visible?
  - **box_2d**: Bounding box in [ymin, xmin, ymax, xmax] format. Only include when video is true. The box should tightly bound the participant's video feed area (including their name label if overlaid on the video).
- **screen_share**: Set to null if no screen sharing is active, otherwise an object containing:
  - **box_2d**: Bounding box for the shared screen content area in [ymin, xmin, ymax, xmax] format. The box should bound the actual shared content area, excluding meeting controls and participant thumbnails.
  - **presenter**: Name of person presenting if identifiable, otherwise null
  - **description**: Brief 1-2 sentence description of what's being shared
  - **formatted_text**: Complete text extraction from the presented screen/slide, formatted in markdown. Preserve structure with headings, bullets, code blocks, etc.

Focus on accuracy. If information isn't visible or is unclear, use "unknown" or null.

Return the JSON object with dict participants; do not use bare name strings.
