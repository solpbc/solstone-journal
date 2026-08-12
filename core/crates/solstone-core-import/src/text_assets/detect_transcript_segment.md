---
context: observe.detect.segment
label: Segmentation
group: Import
---
Split a transcript into ~5-minute segments.

INPUT:
- First line: "START_TIME: HH:MM:SS"
- Remaining lines: numbered "N: content"

OUTPUT: JSON object: {"segments": [{"start_at": "HH:MM:SS", "line": N}, ...]}

RULES:
1. First segment is always {"start_at": START_TIME, "line": 1}
2. If the transcript has timestamps, use them to find ~5-minute boundaries. Add relative timestamps (00:05:30) to START_TIME to get absolute times.
3. If the transcript has NO timestamps, follow these steps:
   a. Count the total lines in the transcript
   b. Divide by 100 to get the number of segments (round up, minimum 2)
   c. Space segments roughly evenly by line count
   d. Adjust each boundary to the nearest topic or speaker change
   e. Space the "start_at" times evenly across 5 minutes per segment from START_TIME
4. All "start_at" times must be absolute HH:MM:SS
5. Do NOT put boundaries in the middle of someone speaking

Return only the JSON object with a top-level "segments" array.
