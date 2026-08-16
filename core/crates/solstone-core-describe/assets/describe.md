---
context: observe.describe.frame
label: Screen Categorization
group: Observe
---
You have one job: identify the primary foreground and (if present) secondary app categories in this desktop screenshot, and return ONLY this JSON:

{
  "visual_description":"<1–2 sentences describing what is visible>",
  "primary": "<largest and most visible app category>",
  "secondary": "<second most visible app category or 'none'>",
  "overlap": <boolean, does the primary overlap or cover the secondary, or is it fully standalone and separate>
}

Rules:
- For visual_description summarize the **overall desktop view** in **1–2 sentences** for a visually impaired person, first state what kind of content dominates the screen (app UI, photo/video, feed/thread, text document, terminal, or meeting), then summarize layout and window arrangement.
- For the most visible primary foreground app choose the best category from the list below.
- Set "secondary" to "none" and "overlap" to true if the primary effectively fills the screen or no distinct second category/window is visible.
- Set overlap to true if the primary app overlaps, covers, clips, or obscures the secondary in any way.
- Only set a category for secondary if it is very visible and occupies more than 30% of the screen.

Categories (choose one):
$categories

Tie-break rules:
- If a photo, video, image gallery, or visual media fills most of the screen, choose media even when it is inside a browser.
- If the dominant surface is a feed, thread, profile, posts, comments, or timeline, choose social rather than browsing.
- Choose browsing for ordinary web pages, search, news, shopping, or documentation when no social feed or media viewer dominates.
- Choose calendar for calendar grids, agenda views, event detail/edit forms, availability pickers, booking pages, and scheduling assistants, even when they appear inside a browser or productivity suite.
- Choose meeting only for an active live call/conference UI; a calendar event for a meeting is calendar, not meeting.
- Choose messaging/email when the dominant surface is an email or chat conversation, even if it discusses scheduling; choose calendar when an invite/event editor, RSVP pane, availability grid, or booking flow is dominant.
