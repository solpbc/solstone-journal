# Story inputs and screen notes

Work and Conversation, the activity Story talents, keep their original activity
input when it fits the floor's input budget: the floor window ([THINK.md](THINK.md))
less the reply allowance and the instructions, counted with the journal's own
tokenizer. When it does not fit, preparation sends a smaller input instead,
regardless of the serving backend's window, so a Story reads the same input on
every backend. The smaller input
holds chronological saved Sense summaries and topics, plus Screen narratives.
Kept native text stays whole: transcripts, voice labels and private memory are
never trimmed within a segment. Missing or
unusable Screen results leave that segment's raw percepts in place. Identity,
facet context, history and instructions are unchanged.

Work shows every segment's own text or none. Conversation keeps whole segments
of its own text, as many as fit, spread evenly across the activity, so who said
what, and in what order, survives inside each kept segment. Every other segment
is told by its reports alone. When the reports alone do not fit, one length cap, the
largest that fits, applies to every Screen narrative: short ones stay whole, and
a long one is cut at a sentence. Sense summaries and
topics stay whole. Past that, the Generate boundary's ordinary fitting is the
last-resort guard, and it discloses what it drops as `partial_input`.

Conversation also reads, ahead of its instructions, a count of speech lines per
speaker label, built by code. The owner is often a minor speaker in their own
conversations, and the count gives the model that proportion directly instead of
leaving it to inference.

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

Conversation's voice gates and whole-activity source-body disqualification remain in force.
The separate [source passages](SOURCE_PASSAGES.md) capability is unchanged.
