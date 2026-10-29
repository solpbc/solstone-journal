# Work Story inputs and screen notes

Work keeps its original activity input when that input fits with the requested
reply allowance. For a constrained local or confidential endpoint, preparation
also supplies a smaller first text part. The Generate boundary chooses it before
trimming the original input. It contains chronological saved Sense summaries and
topics, plus Screen narratives. Native transcripts, voice labels and private
memory stay intact. Missing or unusable Screen results leave that segment's raw
percepts in place. Identity, facet context, history and instructions are unchanged.

That order, the original input first and saved segment results only when it does
not fit, is the current behavior. The direction is the reverse
([THINK.md](THINK.md) § Talent inputs): Work reads saved segment results and
excerpts copied by code from the sources by default, and raw segment data only
where it is small.

Bundled Linux admission uses the native exact chat-body count. Other local
admission uses its existing tokenizer, and an endpoint uses its existing token
estimate and discovered or calibrated window. A provider context refusal refits
against the smaller effective window and reconsiders the alternative first.
The alternative can still need fitting: this does not guarantee whole activity
coverage or repair missing or clipped segment results. Cloud API lanes keep the
original input. This policy adds no completion and changes no provider choice.

When the smaller input is selected, the stored Story has `input_projection`
construction metadata. `partial_input` continues to disclose actual source
clipping. Choosing processed reports alone does not fabricate dropped-entry
counts. Reports retain their missing and partial states; a report is not original
evidence or proof of who acted.

## Literal screen notes

After the one normal Work completion, code can append complete stored Messaging
record values as quoted notes. These are fallible screen-extraction records,
not verified messages or a claim to contain the underlying full email.

For each Story topic, the selector requires informative word overlap and a
request or status cue. It ranks distinct cue count, then distinct topic-word
overlap, then first occurrence. The winning value must share at least two
distinct informative topic words. Duplicate winners share a note; a rejected
winner does not substitute a runner-up. At most five distinct values appear.

An optional note is withheld when its text begins with another different stored
subject in the same app, thread and view anywhere in the activity. The witness
subject must be nonempty and contain no literal ellipsis, and the prefix must
end at a word boundary. This conservative check also withholds some legitimate
forwarded or repeated content. It does not repair or reassign a source record.

The notes are labelled unverified. Their recording time and app are separate
from the stored message date; a missing message date is explicit. Subjects and
text stay separate, with Markdown syntax fenced and unsafe controls displayed
as escapes. Sender text is escaped. Code neither paraphrases the source nor
turns a recording time into a sent date. The complete body must fit 4,000 Unicode
scalar values; otherwise the original generated body is kept without notes.

`story.source_notes` retains source hashes, original fields and every occurrence
alias for the notes actually appended. Regeneration replaces this provenance
with the body, so a later suppressed supplement leaves no stale notes. The
existing checked activity writer protects an owner's held Story as one field.
Model output cannot supply these provenance fields.

The optional supplement requires the complete typed source catalog captured
during preparation. A source file above the native 64 KiB snapshot limit or
unavailable source metadata suppresses the supplement; it does not stop the
Story or the smaller input path. Topic matching is lexical, and useful details
can be missed. Literal copying preserves the stored wording, including any
upstream extraction error.

Conversation keeps its existing inputs and behavior. The
voice gates and whole-activity source-body disqualification remain in force.
The separate [source passages](SOURCE_PASSAGES.md) capability is unchanged.
