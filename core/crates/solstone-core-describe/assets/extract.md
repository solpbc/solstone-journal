---
context: observe.extract.selection
label: Frame Selection
group: Observe
---
You are analyzing frame categorizations from a desktop screencast recording to select frames for detailed content extraction.

Given a time-series of frames with categories and visual descriptions, select the frames most valuable for text extraction and content analysis. Aim to select around $max_extractions frames total, fewer if the content is repetitive.

## Understanding the Input

Each frame has:
- **primary/secondary**: App category (what type of application is visible)
- **visual_description**: A brief description of the desktop layout and visible apps (NOT the actual text content)

Important: The visual descriptions describe window layout and app types, not detailed content. You cannot see actual text, scroll position, or specific content changes - only which apps are visible and how they are arranged.

## Selection Strategy

Select frames that represent distinct screen states:
- Category transitions (e.g. meeting → terminal, messaging → code)
- Different applications appearing (Slack → Gmail, Zoom → Webex)
- Layout changes (split screen → fullscreen, different secondary app)
- First frame of a new activity or context

Skip frames where:
- Same apps in same layout with similar descriptions
- Descriptions differ only in minor wording, not actual content
- Repetitive states (e.g. "Terminal session '0' with 1 pane" repeated)

## Category-Specific Rules

$extraction_guidance

## Input/Output

Input: JSON array of frame objects with frame_id, timestamp, primary, secondary, overlap, and visual_description.

Output: JSON array of selected frame_ids only.

Example: [1, 15, 42, 89]
