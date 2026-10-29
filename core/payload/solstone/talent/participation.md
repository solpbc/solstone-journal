{
  "type": "generate",
  "title": "Participation",
  "description": "Consolidates per-segment Sense entity drafts into a structured per-activity participation list.",
  "hook": {"post": "participation"},
  "schedule": "activity",
  "activities": ["*"],
  "priority": 10,
  "output": "json",
  "schema": "participation.schema.json",
  "max_output_tokens": 6912,
  "timeout_s": 480,
  "load": {
    "transcripts": true,
    "percepts": true,
    "talents": {
      "sense": true
    }
  }
}

$facets

$activity_context

$activity_preamble

# Participation

List the people who took part in this activity with you, and the people it named. The activity above holds its speech, with each line's speaker where the journal recognized the voice, a sample of what its screens and terminals showed, and the people Sense named in each segment. Base every entry on it.

## Who goes in the list

- **People only.** List individual human beings. Never list a company, organization, team, product, project, tool, AI assistant or coding agent, office or job title, place, event, email address or account. Sense sometimes calls one of these a person: judge from what the activity shows about it.
- **Never you.** You are the journal owner. Leave out your own name, every shortening or nickname of it, and "you".
- **Numbered voices.** A voice the speech labels with a number ("Speaker" and a number) is a person the journal has not named yet: list it as an attendee, under that exact label, when it spoke in a meeting or call that is this activity. Leave numbered voices out of anything else. Any other label, role word or placeholder is not a name.
- **Only this activity.** The segments can hold other things happening at the same time. Keep a person only when what they did or how they came up belongs to this activity as its description tells it; leave out people from a show, video or broadcast playing nearby, from another conversation, or from other work at the same time.
- **Nothing invented.** Every name you write must appear in the activity. Never complete a first name into a full name the activity does not give.
- **One entry per person.** When two names in the activity are the same person (a short form, a misspelling, initials), write one entry under the fullest correct form. A first name alone and a full name that begins with it, in the same activity, are one person unless the activity shows two.

## role

- `attendee`: present in this activity with you. They were heard speaking in it, introduced themselves, or were shown as a participant of the call or meeting. A voice from a recording, video, talk on a screen or broadcast is not present.
- `mentioned`: named in this activity without taking part in it. They were talked about, wrote or received a message you read or wrote, wrote something you read, or their name was on a page.

Only a meeting or call has attendees. Speaker recognition can be wrong: a name it put on only one or two lines is an attendee only when something else in the activity (their name said to them, an introduction, Sense naming them in the meeting) supports it. When unsure, write `mentioned`.

## source

Where the strongest evidence came from: `voice` (speaker recognition heard them), `speaker_label` (a participant name in the meeting), `transcript` (someone said their name), `screen` (their name was only written on screen), `other`.

## confidence and context

`confidence` is a number from 0 to 1: how firmly the activity places this person in this activity in this role. `participation_confidence` is the same for the list as a whole.

`context` is one short clause about how this person came up in the activity, drawn from the activity. Write it to the journal owner as "you". The software never speaks as "I", "we" or "my". Write the product name in lowercase: "solstone". Never write "capture" in any form, and never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils.

## Output

```json
{
  "participation": [
    {
      "name": "<a person's name as the activity writes it>",
      "role": "attendee|mentioned",
      "source": "voice|speaker_label|transcript|screen|other",
      "confidence": 0.0,
      "context": "<one short clause>",
      "entity_id": null
    }
  ],
  "participation_confidence": 0.0
}
```

`entity_id` is always `null`. When no other person took part in or was named in this activity, the right answer is `{"participation": [], "participation_confidence": 1.0}`.

Return only the JSON object.
