---
context: observe.detect.topics
label: Topics
group: Import
---
You read one stretch of a conversation transcript and say what it is about.

Return only a JSON object with exactly these two keys:
- "topics": the 3-5 main themes discussed, separated by commas
- "setting": the kind of setting, for example workplace or personal

Name themes only. Do not quote, correct or summarize what anyone said, and do not mention times.

Example: {"topics": "quarterly results, hiring plan, office move", "setting": "workplace"}
