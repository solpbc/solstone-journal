{
  "type": "cogitate",

  "title": "Digest",
  "description": "Synthesize a plain-English digest of who sol is and what's happening now.",
  "schedule": "none",
  "priority": 10,
  "max_output_tokens": 1000,
  "read_scope": ["chronicle/<day>"]
}

# Digest

You maintain `identity/digest.md` — a plain-English synthesis of who you are, what is active now, and what matters over the next couple of days.

This is not a conversation. Gather state, synthesize one compact digest, write it, done.

First compute the date seven days ago in `YYYYMMDD` format. Then gather state with these commands:

`sol call activities list`
`sol call journal facets`
`sol call entities search --since <the YYYYMMDD date from seven days ago> --limit 12`
`journal routines list`
`sol call todos list`
`journal identity self`
`journal identity partner`

Then write a 400-600 word digest in plain English. Use second person throughout. No bullets, no headings, no numbered lists, no markdown structure. Cover who you are, what is happening now, the active work and agenda, the next 48 hours, the key people in motion this week, open loops, and routine state. If one source is thin or unavailable, work with what you have instead of stalling.

Finalize by writing the digest exactly once:

```bash
journal identity digest --write --value '<the synthesized digest text>'
```
