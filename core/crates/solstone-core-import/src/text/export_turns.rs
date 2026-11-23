// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{ExportTime, Reading, Turn};
use crate::structured_exports::{self, StructuredTiming, StructuredTurn};
use regex::Regex;
use std::sync::LazyLock;

fn turn(speaker: Option<&str>, text: &str) -> Turn {
    Turn {
        speaker: speaker.map(str::to_owned),
        recipient: None,
        text: text.to_owned(),
        offset: 0,
        stated: None,
        exporter_notice: false,
        export_time: ExportTime::Unspecified,
    }
}

fn from_structured(parsed: Vec<StructuredTurn>) -> Vec<Turn> {
    parsed
        .into_iter()
        .map(|row| Turn {
            speaker: row.speaker,
            recipient: None,
            text: row.text,
            offset: row.offset,
            stated: None,
            exporter_notice: false,
            export_time: match row.timing {
                StructuredTiming::Sent(instant) => ExportTime::Sent(instant),
                StructuredTiming::NoSendInstant => ExportTime::NoSendInstant,
                StructuredTiming::Caption { start, end } => ExportTime::Caption { start, end },
            },
        })
        .collect()
}

pub(super) fn read(text: &str, reasons: &[&str]) -> Option<Reading> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let turns = if reasons.contains(&"webvtt_cues") {
        captions(text)?
    } else if reasons.contains(&"slack_text") {
        slack(text)?
    } else if reasons.contains(&"whatsapp_bracketed") || reasons.contains(&"whatsapp_dash") {
        whatsapp(text, reasons.contains(&"whatsapp_bracketed"))?
    } else if reasons.contains(&"whisper_tsv") {
        from_structured(structured_exports::parse_whisper_tsv(text)?)
    } else if reasons.contains(&"whisper_json") {
        from_structured(structured_exports::parse_whisper_json(text)?)
    } else if reasons.contains(&"macwhisper_json") {
        from_structured(structured_exports::parse_macwhisper_json(text)?)
    } else if reasons.contains(&"podcast_transcript_json") {
        from_structured(structured_exports::parse_podcast_transcript_json(text)?)
    } else if reasons.contains(&"telegram_json") {
        from_structured(structured_exports::parse_telegram_json(text)?)
    } else if reasons.contains(&"telegram_html") {
        from_structured(structured_exports::parse_telegram_html(text)?)
    } else if reasons.contains(&"slack_json") {
        from_structured(structured_exports::parse_slack_json(text)?)
    } else if reasons.contains(&"fireflies_csv") {
        from_structured(structured_exports::parse_fireflies_csv(text)?)
    } else {
        return None;
    };
    (!turns.is_empty()).then(|| Reading {
        clock_start: None,
        untimed: turns.iter().all(|turn| {
            matches!(
                turn.export_time,
                ExportTime::Unspecified | ExportTime::NoSendInstant
            )
        }),
        turns,
    })
}

fn captions(text: &str) -> Option<Vec<Turn>> {
    static TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"^((?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3})[ \t]+-->[ \t]+((?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3})(?:[ \t]+.*)?$"
    ).unwrap()
    });
    let lines: Vec<_> = text.lines().collect();
    let mut blocks = lines
        .split(|line| line.trim().is_empty())
        .filter(|block| !block.is_empty());
    let header = blocks.next()?.first()?;
    if header.split_ascii_whitespace().next()? != "WEBVTT" {
        return None;
    }
    let mut turns = Vec::new();
    for block in blocks {
        if matches!(block[0], "STYLE" | "REGION" | "NOTE") || block[0].starts_with("NOTE ") {
            continue;
        }
        let index = usize::from(!block[0].contains("-->"));
        let times = TIME.captures(block.get(index)?)?;
        let start = times.get(1)?.as_str();
        let end = times.get(2)?.as_str();
        let millis = |time: &str| -> Option<u64> {
            let (clock, fraction) = time.split_once('.')?;
            let seconds = clock.split(':').try_fold(0u64, |acc, part| {
                acc.checked_mul(60)?.checked_add(part.parse().ok()?)
            })?;
            seconds
                .checked_mul(1000)?
                .checked_add(fraction.parse().ok()?)
        };
        let from = millis(start)?;
        if from >= millis(end)? {
            return None;
        }
        let payload = block.get(index + 1..)?.join("\n");
        if payload.is_empty() {
            return None;
        }
        let voice = payload
            .strip_prefix("<v ")
            .and_then(|s| s.split_once('>'))
            .filter(|(name, body)| {
                !name.trim().is_empty()
                    && !name.contains('<')
                    && !body.strip_suffix("</v>").unwrap_or(body).contains("</v>")
                    && !body.contains("<v ")
            })
            .map(|(name, body)| (name.trim(), body.strip_suffix("</v>").unwrap_or(body)));
        let mut row = match voice {
            Some((name, body)) => turn(Some(name), body),
            None => turn(None, &payload),
        };
        row.offset = from / 1000;
        row.export_time = ExportTime::Caption {
            start: start.to_string(),
            end: end.to_string(),
        };
        turns.push(row);
    }
    Some(turns)
}

fn whatsapp(text: &str, bracketed: bool) -> Option<Vec<Turn>> {
    let mut turns: Vec<Turn> = Vec::new();
    for line in text.lines() {
        let header = crate::triage::strip_direction_marks(line);
        if let Some(row) = crate::triage::whatsapp_row(header, bracketed) {
            // A dated event such as a membership notice has no sender.
            let (speaker, body) = match row.split_once([':', '：']) {
                Some((name, text)) if !name.trim().is_empty() => {
                    (Some(name.trim()), text.strip_prefix(' ').unwrap_or(text))
                }
                _ => (None, row),
            };
            let mut message = turn(speaker, body);
            message.exporter_notice = speaker.is_none() && crate::triage::whatsapp_notice(body);
            turns.push(message);
        } else {
            // The classifier removes bidi controls while recognizing a stamp.
            // Do not attribute a partly decoded header to the previous sender.
            if line.contains(crate::triage::BIDI_MARKS)
                && crate::triage::whatsapp_row(
                    &line.replace(crate::triage::BIDI_MARKS, ""),
                    bracketed,
                )
                .is_some()
            {
                return None;
            }
            let previous = turns.last_mut()?;
            previous.text.push('\n');
            previous.text.push_str(line);
        }
    }
    Some(turns)
}

fn slack(text: &str) -> Option<Vec<Turn>> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"^(?P<nesting>(?:\|\s*)*)>\s+(?P<speaker>[^\[\r\n]{1,60}?)\s+\[[A-Z0-9]+\]\s+@\s+(?P<stamp>[0-9]{2}/[0-9]{2}/[0-9]{4}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}\s+(?:Z|[+-][0-9]{4})):$"
    ).unwrap()
    });
    let mut turns = Vec::new();
    let mut stamps = Vec::new();
    let mut body = Vec::new();
    let mut sender = None;
    let mut depth = 0;
    let finish = |sender, body: &mut Vec<&str>, turns: &mut Vec<Turn>| -> Option<()> {
        if body.last() == Some(&"") {
            body.pop();
        }
        if body.is_empty() {
            return None;
        }
        turns.push(turn(sender, &body.join("\n")));
        body.clear();
        Some(())
    };
    for line in text.lines() {
        let header = crate::triage::strip_direction_marks(line);
        if let Some(parts) = HEADER.captures(header) {
            if sender.is_some() {
                finish(sender, &mut body, &mut turns)?;
            }
            sender = Some(parts.name("speaker")?.as_str().trim());
            depth = parts.name("nesting")?.as_str().matches('|').count();
            let stamp = parts.name("stamp")?.as_str().replace(" Z", " +0000");
            stamps.push(stamp);
        } else {
            if line.contains(crate::triage::BIDI_MARKS)
                && HEADER.is_match(&line.replace(crate::triage::BIDI_MARKS, ""))
            {
                return None;
            }
            sender?;
            let mut rest = line;
            for _ in 0..depth {
                if rest.is_empty() {
                    break;
                }
                rest = rest.strip_prefix('|')?;
                rest = rest
                    .strip_prefix("   ")
                    .unwrap_or_else(|| rest.strip_prefix(' ').unwrap_or(rest));
            }
            body.push(rest);
        }
    }
    finish(Some(sender?), &mut body, &mut turns)?;
    // Both day/month and month/day occur. A date elsewhere in this same
    // export can resolve the convention. An ambiguous clock supplies no instant.
    let parse = |format, stamp: &str| jiff::Timestamp::strptime(format, stamp).ok();
    let us = "%m/%d/%Y %T %z";
    let eu = "%d/%m/%Y %T %z";
    let supports = |format| stamps.iter().all(|stamp| parse(format, stamp).is_some());
    let orders = (supports(us), supports(eu));
    for (row, stamp) in turns.iter_mut().zip(&stamps) {
        let instant = match orders {
            (true, false) => parse(us, stamp),
            (false, true) => parse(eu, stamp),
            (true, true) if parse(us, stamp) == parse(eu, stamp) => parse(us, stamp),
            _ => None,
        };
        row.export_time = instant.map_or(ExportTime::NoSendInstant, ExportTime::Sent);
    }
    Some(turns)
}

#[cfg(test)]
mod tests {
    use super::super::{jsonl_rows, reading_for};
    use serde_json::{Value, json};
    use solstone_core_format::content::conversation::{
        ConversationMedium, imported_text_medium, message_blocks, message_fields,
    };
    use std::path::Path;

    fn rows(name: &str, input: &str) -> (ConversationMedium, Vec<Value>) {
        let verdict = crate::triage::classify(input.as_bytes(), name, Path::new(""), None, None);
        let reading = reading_for(input, &verdict, "import.text");
        let rows = jsonl_rows(
            &reading.turns,
            0,
            jiff::Timestamp::UNIX_EPOCH,
            "fixture",
            name,
            &verdict,
            None,
            None,
            None,
            reading.untimed,
        );
        (imported_text_medium(&rows[0], None), rows)
    }

    #[test]
    fn written_export_rows_keep_message_bodies_and_only_proven_instants() {
        let (medium, w) = rows(
            "phone.txt",
            "[11/17/26, 13:04:12] Mira: Bringing the cart.\n\nTwo shelves.\n[11/17/26, 13:05:40] Theo: Thanks.",
        );
        assert_eq!(medium, ConversationMedium::Unresolved);
        assert_eq!(w.len(), 3);
        assert_eq!(w[1]["speaker"], "Mira");
        assert_eq!(w[1]["text"], "Bringing the cart.\n\nTwo shelves.");
        assert_eq!(w[0]["untimed"], true);
        let (medium, d) = rows(
            "phone.txt",
            "8/17/25, 9:01 AM - Messages and calls are end-to-end encrypted.\n8/17/25, 9:01 AM - Pia: Where is it?\n8/17/25, 9:02 AM - Ivo: At the shop.\n",
        );
        assert_eq!(medium, ConversationMedium::Written);
        assert!(d[1]["speaker"].is_null());
        assert_eq!(d[3]["speaker"], "Ivo");
        let (medium, z) = rows(
            "meeting.txt",
            "10:02:11 From Theo to Everyone:\n\tHere are the shelves.\n10:03:05 From Mira to Theo:\n\t\tcode",
        );
        assert_eq!(medium, ConversationMedium::Written);
        assert_eq!(z[1]["text"], "Here are the shelves.");
        assert_eq!(z[2]["text"], "\tcode");
        assert_eq!(
            message_blocks(z[1]["text"].as_str().unwrap())[0]["kind"],
            "paragraph"
        );
        let text = "> Mira Alder [U01] @ 11/17/2026 14:10:05 -0700:\nwho has the cart?\n\nsecond paragraph.\n\n> Theo Park [U02] @ 11/17/2026 14:11:40 -0700:\nI do.";
        let (medium, s) = rows("slack.txt", text);
        assert_eq!(medium, ConversationMedium::Written);
        assert_eq!(s.len(), 3);
        let fields =
            message_fields("import.text", "chat", 0, s[1].as_object().unwrap(), false).unwrap();
        assert_eq!(fields["author"], "Mira Alder");
        assert_eq!(fields["sent_at"], "2026-11-17T21:10:05Z");
        assert_eq!(s[1]["text"], "who has the cart?\n\nsecond paragraph.");
        let (_, a) = rows("slack.txt", &text.replace("11/17/", "11/10/"));
        assert_eq!(a[0]["untimed"], true);
        assert_eq!(a[1].get("sent_at"), Some(&Value::Null));
        let mixed = text
            .replacen("11/17/", "11/11/", 1)
            .replace("11/17/", "11/10/");
        let (_, mixed_rows) = rows("slack.txt", &mixed);
        assert!(mixed_rows[1]["sent_at"].is_string());
        assert_eq!(mixed_rows[2].get("sent_at"), Some(&Value::Null));
        let quoted = format!(
            "[11/17/26, 13:04:12] Mira: From the manual\nMessages and calls are end-to-end encrypted.\n[11/17/26, 13:05:40] Theo: {}",
            "a".repeat(9000)
        );
        let (medium, _) = rows("phone.txt", &quoted);
        assert_eq!(medium, ConversationMedium::Unresolved);
        let row = json!({"speaker":"You","text":"cart","is_owner":true});
        let native =
            message_fields("import.chatgpt", "chat", 0, row.as_object().unwrap(), false).unwrap();
        assert_eq!(native["role"], "user");
        assert!(native.get("author").is_none());
    }

    #[test]
    fn ambiguous_dated_dialogue_never_becomes_written_from_the_rule_name() {
        for version in [1, 2, 7] {
            let header = json!({"imported":{"id":"old","triage":{"kind":"transcripts","ruleset_version":version,"reasons":["whatsapp_bracketed","signal_dated"]}}});
            assert_eq!(
                imported_text_medium(&header, None),
                ConversationMedium::Unresolved
            );
        }
        let (_, z) = rows(
            "meeting.txt",
            "10:02:11 From Theo to Everyone: Before
\tcode
10:03:05 From Mira to Theo: After",
        );
        assert_eq!(z[1]["text"], "Before\n\tcode");
    }
    #[test]
    fn webvtt_cues_keep_their_relative_clock_and_source_voice() {
        let (medium, rows) = rows(
            "meeting.vtt",
            "WEBVTT\n\n1\n00:00:00.000 --> 00:00:07.500\n<v Mira Alder>One line.\nAnother.</v>\n\nnext\n00:08.125 --> 00:12.000\nNo speaker.\n",
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["speaker"], "Mira Alder");
        assert_eq!(rows[1]["text"], "One line.\nAnother.");
        assert_eq!(rows[1]["cue_start"], "00:00:00.000");
        assert_eq!(rows[2]["cue_start"], "00:08.125");
        assert_eq!(rows[2]["cue_end"], "00:12.000");
        assert!(rows[2]["speaker"].is_null());
    }

    #[test]
    fn structured_exports_preserve_speakers_order_times_and_mediums() {
        let (medium, tg_json) = rows(
            "result.json",
            r#"{"name":"Harbor","type":"personal_chat","id":1,"messages":[
                {"id":1,"type":"message","date":"2026-04-01T08:05:00","date_unixtime":"1775030700","from":"Mira","from_id":"user101","text":"Did the varnish dry?"},
                {"id":2,"type":"message","date":"2026-04-01T08:06:10","date_unixtime":"1775030770","from":null,"from_id":"user202","text":["Almost, check ",{"type":"bold","text":"port side"},"."]}
            ]}"#,
        );
        assert_eq!(medium, ConversationMedium::Written);
        assert_eq!(tg_json.len(), 3);
        assert_eq!(tg_json[0]["first_sent_at"], "2026-04-01T08:05:00Z");
        assert_eq!(tg_json[0]["last_sent_at"], "2026-04-01T08:06:10Z");
        assert_eq!(tg_json[1]["speaker"], "Mira");
        assert_eq!(tg_json[1]["sent_at"], "2026-04-01T08:05:00Z");
        assert_eq!(tg_json[2]["speaker"], "user202");
        assert_eq!(tg_json[2]["text"], "Almost, check port side.");

        let (medium, tg_html) = rows(
            "messages.html",
            r#"<!DOCTYPE html><html><body><div class="page_body chat_page"><div class="history">
            <div class="message default clearfix" id="message1"><div class="body">
            <div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div>
            <div class="from_name">Mira</div>
            <div class="text">Use &amp;lt;tag&amp;gt; &amp; check.<br>Line two.</div>
            </div></div>
            <div class="message default clearfix joined" id="message2"><div class="body">
            <div class="pull_right date details" title="01.04.2026 11:06:10 UTC+03:00">11:06</div>
            <div class="text">Second burst.</div>
            </div></div></div></div></body></html>"#,
        );
        assert_eq!(medium, ConversationMedium::Written);
        assert_eq!(tg_html.len(), 3);
        assert_eq!(tg_html[1]["speaker"], "Mira");
        assert_eq!(tg_html[1]["sent_at"], "2026-04-01T08:05:00Z");
        // Single-pass entity decode: &amp;lt; becomes literal &lt;, never <.
        assert_eq!(tg_html[1]["text"], "Use &lt;tag&gt; & check.\nLine two.");
        assert_eq!(tg_html[2]["speaker"], "Mira");
        assert_eq!(tg_html[2]["text"], "Second burst.");

        let (medium, slack_ids) = rows(
            "chan.json",
            r#"[{"type":"message","user":"U111AAA","text":"Gate open.","ts":"1743494400.000100"},{"type":"message","user":"U222BBB","text":"Heading in.","ts":"1743494460.000200","user_profile":{"real_name":"Theo Park"}}]"#,
        );
        assert_eq!(medium, ConversationMedium::Written);
        assert_eq!(slack_ids.len(), 3);
        assert_eq!(slack_ids[1]["speaker"], "U111AAA");
        assert_eq!(slack_ids[2]["speaker"], "Theo Park");
        assert!(slack_ids[1]["sent_at"].is_string());

        let (medium, ff_csv) = rows(
            "retro.csv",
            "Speaker,Start Time,End Time,Sentence\r\nMira,00:00:01,00:00:03,\"Hello, \"\"team\"\"\"\r\nTheo,00:00:05,00:00:08,Next item.\r\n",
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(ff_csv.len(), 3);
        assert_eq!(ff_csv[1]["speaker"], "Mira");
        assert_eq!(ff_csv[1]["text"], "Hello, \"team\"");
        assert_eq!(ff_csv[1]["cue_start"], "00:00:01");
        assert_eq!(ff_csv[1]["cue_end"], "00:00:03");

        let (medium, mw) = rows(
            "meeting.json",
            r#"{"text":"Hi. Bye.","segments":[{"id":"11111111-2222-3333-4444-555555555555","start":0,"end":1600,"text":"Hi.","speaker":"Speaker 1"},{"id":"66666666-7777-8888-9999-aaaaaaaaaaaa","start":1700,"end":3200,"text":"Bye.","speaker":"Speaker 2"}]}"#,
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(mw.len(), 3);
        assert_eq!(mw[1]["speaker"], "Speaker 1");
        assert_eq!(mw[1]["cue_end"], "00:00:01.600");

        let (medium, pod) = rows(
            "ep.json",
            r#"{"version":"1.0.0","language":"en","segments":[{"speaker":"Mira","startTime":0.0,"endTime":1.0,"body":"Good "},{"speaker":"Mira","startTime":1.0,"endTime":2.0,"body":"morning."},{"speaker":"Theo","startTime":2.2,"endTime":3.5,"body":"Morning."}]}"#,
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(pod.len(), 3);
        assert_eq!(pod[1]["speaker"], "Mira");
        assert_eq!(pod[1]["text"], "Good morning.");
        assert_eq!(pod[1]["cue_start"], "00:00:00.000");
        assert_eq!(pod[1]["cue_end"], "00:00:02.000");
        assert_eq!(pod[2]["speaker"], "Theo");

        let (medium, wj) = rows(
            "whisper.json",
            r#"{"text":"Ready.","language":"en","segments":[{"start":1.5,"end":3.25,"text":"Ready."}]}"#,
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(wj.len(), 2);
        assert_eq!(wj[1]["cue_start"], "00:00:01.500");
        assert_eq!(wj[1]["cue_end"], "00:00:03.250");

        let (medium, wt) = rows(
            "whisper.tsv",
            "start\tend\ttext\n0\t1200\tFirst line.\n1300\t2600\tSecond line.\n",
        );
        assert_eq!(medium, ConversationMedium::Spoken);
        assert_eq!(wt.len(), 3);
        assert_eq!(wt[1]["cue_start"], "00:00:00.000");
        assert_eq!(wt[1]["cue_end"], "00:00:01.200");
        assert_eq!(wt[2]["text"], "Second line.");
    }
}
