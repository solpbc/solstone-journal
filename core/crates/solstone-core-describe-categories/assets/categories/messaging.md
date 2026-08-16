{

  "description": "Chat or email apps (Slack, Discord, Messages/iMessage, Gmail, etc.)",
  "output": "json",
  "extraction": "Extract when conversation partner, channel, or messaging app changes",
  "importance": "high",
  "max_output_tokens": 8192

}

# Messaging Extraction

Extract structured text from this messaging or email screenshot (Slack, Discord, Messages, Gmail, Teams, etc.).

Return JSON matching this shape:

```json
{
  "app": "Gmail",
  "thread": "Inbox",
  "view": "inbox",
  "messages": [
    {
      "sender": "Alice",
      "timestamp": "2:34 PM",
      "subject": "Project update",
      "text": "Latest visible message or email snippet"
    }
  ]
}
```

## Field Notes

- Set `app` to the visible app or service name.
- Set `thread` to the visible channel, conversation, inbox, or list name.
- Set `view` to `inbox` for email/message list views, `conversation` for threaded chats, and `unknown` only when the surface is ambiguous.
- Preserve conversation order and flow in `messages`.
- For inbox rows, put the email subject line in `subject` and the snippet/body preview in `text`.
- For conversations, set `subject` to null.
- Put timestamps in `timestamp` when visible; otherwise use null.
- `text` may contain markdown. Use `>` blockquotes for quoted/forwarded content and code fences for code snippets.
- Mark unclear text with `[unclear]` inside `text`.
- Mark cut-off text with `...` inside `text`.
- Focus on message content and skip unrelated UI chrome.

Return ONLY the JSON object.
