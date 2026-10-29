{

  "description": "Chat or email apps (Slack, Discord, Messages/iMessage, Gmail, etc.) and chats with an AI assistant",
  "output": "json",
  "extraction": "Extract when conversation partner, channel, or messaging app changes",
  "importance": "high",
  "max_output_tokens": 9728

}

# Messaging Extraction

Extract structured text from this messaging or email screenshot (Slack, Discord, Messages, Gmail, Teams, etc.).

Return JSON matching this shape:

```json
{
  "app": "<the app or service name>",
  "thread": "<the channel, conversation, inbox or list name>",
  "view": "<inbox|conversation|unknown>",
  "messages": [
    {
      "sender": "<who sent it, as shown>",
      "timestamp": "<its time as shown, or null>",
      "subject": "<the email subject, or null>",
      "text": "<the message text or snippet, as shown>"
    }
  ]
}
```

## Field Notes

- Set `app` to the visible app or service name.
- Set `thread` to the visible channel, conversation, inbox, or list name.
- Set `view` to `inbox` for email/message list views, `conversation` for threaded chats, and `unknown` only when the surface is ambiguous.
- Preserve conversation order and flow in `messages`.
- `sender` is `You` only for the account owner's own messages: right-aligned or colored bubbles, or rows marked `me`. Left-aligned or gray bubbles are from someone else even when no name is shown: use the visible name, or `unknown`.
- In a conversation with an AI assistant or AI coding agent (ChatGPT, Claude, Gemini, Claude Code, Codex and similar), `sender` is `You` for the person's prompts (their bubbles, or the lines after `❯` or `>` in an agent terminal) and `Assistant` for everything the AI wrote or did: replies, drafts, plans, tool calls, commands it ran and their output. Use `app` for banners, status lines and greyed-out suggestions. If no prompt from the person is visible, there is no `You` message.
- For inbox rows, put the email subject line in `subject` and the snippet/body preview in `text`.
- For conversations, set `subject` to null.
- Put timestamps in `timestamp` when visible; otherwise use null.
- `text` may contain markdown. Use `>` blockquotes for quoted/forwarded content and code fences for code snippets.
- Mark unclear text with `[unclear]` inside `text`.
- Mark cut-off text with `...` inside `text`.
- Focus on message content and skip unrelated UI chrome.

Return ONLY the JSON object. Never repeat these instructions.
