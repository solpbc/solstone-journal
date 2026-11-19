// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

const VTT: &str = "WEBVTT\n\n00:01.000 --> 00:02.000\nWind rose.\n";
const SRT: &str = "1\n00:00:01,000 --> 00:00:02,000\nWind rose.\n\n2\n00:00:03,000 --> 00:00:04,000\nLeaves fell.\n";
const INLINE: &str = "[00:00:00] Lio: The bulbs are ready.\n[00:00:10] Val: I can plant them.\n";
const CHAT: &str = "Lio: Is the clay ready?\nVal: It needs water.\nLio: A bucket?\nVal: Half of one.\nLio: I'll fetch it.\nVal: Thank you.\n";
const EMAIL: &str = "From: Ivo <ivo@example.invalid>\nTo: Pia <pia@example.invalid>\nSubject: Clay\n\nI left it by the kiln.\n\n> Is the clay ready?\n";
const PLAY: &str =
    "INT. WORKSHOP - NIGHT\n\nLIO\nThe clay is ready.\n\nVAL\nI'll fetch the bucket.\n";
const EMBEDDED: &str = "# Kiln notes\n\nI am keeping a copy of the chat here alongside the kiln settings.\n\n[00:00:00] Lio: The bulbs are ready.\n[00:00:10] Val: I can plant them.\n\n## Next firing\n\nPack the tools first.\n";

fn verdict(name: &str, data: impl AsRef<[u8]>) -> Verdict {
    classify(data.as_ref(), name, Path::new(""), None, None)
}

fn expected(name: &str, bytes: impl AsRef<[u8]>, kind: Kind, reasons: &[&str]) {
    let found = verdict(name, bytes.as_ref());
    assert_eq!(found.kind, kind, "{name}: {:?}", found.reasons);
    assert_eq!(found.reasons, reasons, "{name}");
    assert_eq!(found.ruleset_version, 4);
    let shuffled = RULES
        .iter()
        .rev()
        .step_by(2)
        .chain(RULES.iter().rev().skip(1).step_by(2));
    assert_eq!(
        classify_with_rules(bytes.as_ref(), name, Path::new(""), None, None, shuffled),
        found,
        "{name}"
    );
}

#[test]
fn each_rule_has_unique_id_and_priority() {
    let mut priorities = HashSet::new();
    let mut ids = HashSet::new();
    for rule in RULES.iter() {
        assert!(priorities.insert(rule.priority));
        assert!(ids.insert(rule.id));
        assert!(!rule.source.is_empty());
    }
}

#[test]
fn explicit_owner_then_known_declaration_then_content() {
    assert_eq!(
        classify(
            VTT.as_bytes(),
            "work",
            Path::new(""),
            Some("photos"),
            Some(Kind::Notes)
        )
        .reasons,
        ["owner_choice"]
    );
    assert_eq!(
        classify(VTT.as_bytes(), "work", Path::new(""), Some("photos"), None).kind,
        Kind::Photos
    );
    assert_eq!(
        classify(VTT.as_bytes(), "work", Path::new(""), Some("unknown"), None),
        verdict("work", VTT)
    );
}

#[test]
fn timed_and_untimed_export_families() {
    let discord = format!(
        "{bar}\nGuild: Potters\nChannel: firing\n{bar}\n\n[4/7/2025 10:02 AM] lio\nReady.\n\n[4/7/2025 10:03 AM] val\nComing.\n\n{bar}\nExported 2 message(s)\n{bar}\n",
        bar = "=".repeat(62)
    );
    for (name, file, rule) in [
        ("cue.vtt", VTT, "webvtt_cues"),
        (
            "zoom.vtt",
            "WEBVTT\n\n1\n00:00:03.000 --> 00:00:04.000\nLio: Hold the hinge.\n",
            "webvtt_cues",
        ),
        (
            "teams.vtt",
            "WEBVTT\n\nclip-7\n00:03.000 --> 00:04.000 align:start\n<v.calm Lio>Hold the hinge.\n",
            "webvtt_cues",
        ),
        ("captions.srt", SRT, "subrip_cues"),
        (
            "whisper.tsv",
            "start\tend\ttext\n0\t120\tAlmost done.\n121\t480\tReady now.\n",
            "whisper_tsv",
        ),
        (
            "whisper.json",
            r#"{"text":"Ready.","language":"en","segments":[{"start":0.1,"end":0.8,"text":"Ready."}]}"#,
            "whisper_json",
        ),
        (
            "whisper.log",
            "[00:00:01.000 --> 00:00:02.000] Wind.\n[00:00:02.000 --> 00:00:03.000] Rain.\n[00:00:03.000 --> 00:00:04.000] Sun.\n",
            "bracketed_offsets",
        ),
        (
            "zoom.txt",
            "10:07:10 From Ivo : Hold it.\n10:07:12 From Pia to Everyone : Holding.\n",
            "zoom_chat",
        ),
        (
            "zoom-block.txt",
            "10:07:10 From Ivo to Everyone:\n\tHold it.\n10:07:12 From Pia to Everyone:\n\tHolding.\n",
            "zoom_chat",
        ),
        (
            "phone.txt",
            "\u{200e}[17/08/2025, 09:01:01] Pia: Where is the shelf?\n[17/08/2025, 09:01:03] Ivo: Around the corner.\nBy the door.\n",
            "whatsapp_bracketed",
        ),
        (
            "android.txt",
            "8/17/25, 9:01 AM - Messages and calls are end-to-end encrypted.\n8/17/25, 9:01 AM - Pia: Where is it?\n8/17/25, 9:02\u{202f}AM - Ivo: At the shop.\n",
            "whatsapp_dash",
        ),
        (
            "imessage.txt",
            "Feb 7, 2025  7:29:42 PM\nMe\nIs the store open?\n\nFeb 7, 2025  7:31:12 PM (Read by them after 5 seconds)\nPia\nYes, until eight.\n",
            "dated_sender_blocks",
        ),
        (
            "signal.md",
            "[2025-08-17 09:01:01] Pia: Need a hand?\n[2025-08-17, 09:02] Ivo: Yes!\n",
            "signal_dated",
        ),
        (
            "sigtop.txt",
            "Conversation: Pia\n\nFrom: You\nType: outgoing\nSent: Tue, 3 Sep 2024 00:00:01 +0000\n\nHere.\n\nFrom: Pia\nType: incoming\nSent: Tue, 3 Sep 2024 00:00:02 +0000\n\nWhere?\n",
            "signal_sigtop_long",
        ),
        (
            "sigtop-short.txt",
            "2025-08-17 09:01 Pia: Need a hand?\n2025-08-17 09:02 Ivo: Yes!\n",
            "signal_sigtop_short",
        ),
        (
            "slack.txt",
            "> Pia [U01] @ 17/08/2025 09:01:01 Z:\nNeed a hand?\n|   \n|   > Ivo [U02] @ 17/08/2025 09:01:03 Z:\n|   Yes!\n",
            "slack_text",
        ),
        ("discord.txt", &discord, "discord_export"),
        (
            "irc.log",
            "--- Log opened Tue Sep 3 00:00:00 2024\n00:00 -!- lio has joined\n00:01 <@lio> Ready.\n00:01 < val> Over here.\n00:02 <@lio> Coming.\n",
            "irc_angle_nick",
        ),
        (
            "dated-tab.log",
            "2025-08-17 09:01:00.123456Z\t-->\tpi joined\n2025-08-17 09:01:01.000000Z\tpi\tGo.\n2025-08-17 09:01:02Z\tivo\tWait.\n2025-08-17 09:01:03\tpi\tOkay.\n",
            "dated_tab_nick",
        ),
        (
            "roles.md",
            "---\ntitle: Kiln\n---\n\n# First firing\n\n#### You:\nIs the glaze dry?\n\n#### ChatGPT:\nIt takes a day.\n",
            "role_headers",
        ),
        ("bare", CHAT, "bare_speaker"),
        (
            "clock.log",
            "[9:01] Lio: Ready?\n[9:02] Val: Yes.\n[9:03] Lio: Go.\n",
            "bracketed_speaker",
        ),
        (
            "clock-dash.log",
            "09:01 - Lio: Ready?\n09:02 - Val: Yes.\n09:03 - Lio: Go.\n",
            "clock_dash_speaker",
        ),
        (
            "stacked.txt",
            "Lio  0:01\nSet the mold.\n\nVal  0:02\nHere it is.\n\nLio  0:06\nPour slowly.\n",
            "stacked_speaker_time",
        ),
        (
            "time-first.txt",
            "0:01 - Lio\nSet it.\n0:03 - Val\nHere it is.\n0:05 - Lio\nPour.\n",
            "clock_then_speaker",
        ),
        (
            "shortclock.txt",
            "Lio (0:01): Set it.\nVal (0:03): Here it is.\nLio (0:05): Pour.\n",
            "speaker_parenthesized_clock",
        ),
        (
            "speaker.txt",
            "Speaker 2: Set it.\nSpeaker 1: Here it is.\nSpeaker 2: Pour.\n",
            "role_labels",
        ),
        (
            "qa.txt",
            "Interview with Sol\n\nQ: Have you met?\nA: At the station.\nQ: When was that?\n",
            "role_labels",
        ),
        (
            "me-them.txt",
            "Me: Ready?\nThem: Almost.\nMe: I can wait.\n",
            "role_labels",
        ),
        (
            "ai.txt",
            "Human: Ready?\nAssistant: Almost.\nHuman: I can wait.\n",
            "role_labels",
        ),
        (
            "childes.cha",
            "@Begin\n@Participants:\tCHI Child, ADU Adult\n*CHI:\tmore clay .\n*ADU:\tthere it is .\n@End\n",
            "childes_chat",
        ),
        (
            "sms.xml",
            "<?xml version=\"1.0\"?><smses count=\"1\"><sms body=\"Take a seat\" /></smses>",
            "sms_backup_xml",
        ),
        ("existing.md", INLINE, "existing_timed_layout"),
    ] {
        expected(name, file, Kind::Transcripts, &[rule]);
    }
    let bold = CHAT
        .lines()
        .map(|line| {
            let (name, message) = line.split_once(':').unwrap();
            format!("**{name}:**{message}\n")
        })
        .collect::<String>();
    expected("bold.md", bold, Kind::Transcripts, &["bold_speaker"]);
}

#[test]
fn ordinary_notes_and_near_misses() {
    for (name, text) in [
        (
            "transcript.txt",
            "quiet under the bridge\nfootsteps on the path\nsoon it will be light",
        ),
        (
            "minutes.md",
            "# Kiln committee\nAttendees: Pia, Ivo\nAgenda: Safety\nAction items:\n- Ivo: Read the manual\n- Pia: Inspect the shelf\n",
        ),
        (
            "todos.txt",
            "Pia: buy nails\nIvo: send invoice\nJun: call the shop\n",
        ),
        (
            "cake.txt",
            "Prep: 25 min\nServes: 6\nIngredients: flour\n\n1. Stir the mixture.\n2. Bake.\n",
        ),
        (
            "frontmatter.md",
            "---\nowner: Pia\nstatus: ready\ntags: [home]\n---\n# Kiln\n",
        ),
        (
            "glossary.txt",
            "Clay: Earth suitable for pottery.\nGlaze: A coating.\nKiln: A heated chamber.\n",
        ),
        (
            "process.log",
            "09:09:14 - INFO: Starting engine\n09:09:15 - WARN: Cooling\n09:09:16 - INFO: Standby\n",
        ),
        (
            "checklist.md",
            "**Status:** done\n**Owner:** Pia\n**Date:** Sunday\n",
        ),
        (
            "bibliography.txt",
            "Alba, F.: Plants of the moon\nBerr, M.: A history of clocks\n",
        ),
        (
            "config.txt",
            "[store]\nsite=desk\nport: 42\nmethod: local\n",
        ),
        (
            "prose.txt",
            "Note: The path is open.\nWarning: The stones are slick.\nUpdate: They dried.\n",
        ),
        (
            "poem.txt",
            "River: Over the hill\nRiver: Under the sill\nSky: Above the birch\nSky: Below the moon\nRiver: Over the hill\n",
        ),
        (
            "diary.log",
            "09:30 - watered the plants\n10:12 - repotted the oak\n",
        ),
        (
            "verse.txt",
            "3:16 The sand is cool.\n3:17 The wind is still.\n",
        ),
        (
            "novel.txt",
            "\"I'm here,\" said Lio.\n\"I know.\" The clay turned slowly.\n",
        ),
        (
            "interview.md",
            "We met in the workshop and discussed its new kiln.\n\nQ: When did you arrive?\nA: This year.\nQ: Will you stay?\n",
        ),
        (
            "one-answer.md",
            "## Assistant\nHere is a list.\n## Instructions\nMix slowly.",
        ),
        (
            "unmarked.txt",
            "meeting starts now\ngardens need rain\ncoffee is ready",
        ),
    ] {
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn email_thread_is_document() {
    expected("saved mail.txt", EMAIL, Kind::Documents, &["email_headers"]);
}

#[test]
fn screenplay_is_document() {
    expected("script.txt", PLAY, Kind::Documents, &["script_scenes"]);
}

#[test]
fn note_with_pasted_timed_chat_stays_note() {
    expected("kiln.md", EMBEDDED, Kind::Notes, &["note_default"]);
    expected(
        "same-excerpt.txt",
        INLINE,
        Kind::Transcripts,
        &["existing_timed_layout"],
    );
    let excerpt_first = format!(
        "# Research notes\n{INLINE}\n## Analysis\n{}",
        "The kiln needs a separate set of tools for testing this clay before it goes in beside the regular mix. ".repeat(5),
    );
    expected("research.md", excerpt_first, Kind::Notes, &["note_default"]);
    let fenced = format!("# Kiln notes\n\n```text\n{CHAT}```\n");
    expected("fenced.md", fenced, Kind::Notes, &["note_default"]);
}

#[test]
fn document_structure_and_size_boundaries() {
    expected(
        "chapters.txt",
        "Chapter I\nProse.\nChapter II\nProse.\nChapter III\nProse.\n",
        Kind::Documents,
        &["chapter_markers"],
    );
    expected(
        "contents.md",
        "# Table of contents\n- Mud\n- Straw\n- Rope\n\n## Mud\nClay.\n## Straw\nHay.\n## Rope\nTwine.\n",
        Kind::Documents,
        &["table_of_contents"],
    );
    expected(
        "ebook.txt",
        "*** START OF THE PROJECT GUTENBERG EBOOK FICTIONAL PROOF ***\n\nA short story.\n",
        Kind::Documents,
        &["gutenberg_header"],
    );
    expected(
        "prose.txt",
        "word ".repeat(4_999),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "prose.txt",
        "word ".repeat(5_000),
        Kind::Documents,
        &["long_prose"],
    );
    let heading = "# Firing\n## Clay\n";
    expected(
        "draft.md",
        format!("{heading}{}", "word ".repeat(1_495)),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "draft.md",
        format!("{heading}{}", "word ".repeat(1_496)),
        Kind::Documents,
        &["structured_prose"],
    );
    expected(
        "tasks.md",
        "- take out the heavy blue bucket and check the handle.\n".repeat(500),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "long-chat.txt",
        CHAT.repeat(250),
        Kind::Transcripts,
        &["bare_speaker"],
    );
}

#[test]
fn formats_bytes_take_precedence_over_names() {
    let png_header = b"\x89PNG\r\n\x1a\n";
    expected(
        "screenshot.png",
        png_header,
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "Screenshot_20250407-123123.png",
        png_header,
        Kind::Screen,
        &["aosp_screenshot_name"],
    );
    expected(
        "Screenshot_20250407-123123-display-2.png",
        png_header,
        Kind::Screen,
        &["aosp_screenshot_name"],
    );
    expected(
        "Screenshot_20250230-123123.png",
        png_header,
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "Screenshot_20250407-123123.png",
        b"not a photo",
        Kind::Notes,
        &["note_default"],
    );
    let at_source = classify(
        png_header,
        "Screenshot_20250407-123123.png",
        Path::new("sdcard/Pictures/Screenshots"),
        None,
        None,
    );
    assert_eq!(
        at_source.reasons,
        ["aosp_screenshot_name", "aosp_screenshot_folder"]
    );
    expected(
        "photo.png",
        b"%PDF-1.7\n",
        Kind::Documents,
        &["document_format"],
    );
}

#[test]
fn container_members_identify_office_bytes_without_a_docx_extension() {
    let office = include_bytes!("../../../../../tests/fixtures/triage-generic-office.zip");
    expected("attachment", office, Kind::Documents, &["document_format"]);
}

#[test]
fn camera_exif_takes_precedence_over_the_screenshot_name() {
    // A minimal little-endian IFD with Make, Model, and ExposureTime fields.
    let mut tiff = b"II*\0\x08\0\0\0\x03\0".to_vec();
    for (tag, field_type, count, value) in [
        (0x010fu16, 2u16, 4u32, u32::from_le_bytes(*b"ABC\0")),
        (0x0110, 2, 4, u32::from_le_bytes(*b"XYZ\0")),
        (0x8769, 4, 1, 50),
    ] {
        tiff.extend(tag.to_le_bytes());
        tiff.extend(field_type.to_le_bytes());
        tiff.extend(count.to_le_bytes());
        tiff.extend(value.to_le_bytes());
    }
    tiff.extend(0u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(0x829au16.to_le_bytes());
    tiff.extend(5u16.to_le_bytes());
    tiff.extend(1u32.to_le_bytes());
    tiff.extend(68u32.to_le_bytes());
    tiff.extend(0u32.to_le_bytes());
    tiff.extend(1u32.to_le_bytes());
    tiff.extend(60u32.to_le_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(u32::try_from(tiff.len()).unwrap().to_be_bytes());
    png.extend(b"eXIf");
    png.extend(tiff);
    png.extend([0u8; 4]);
    add_png_chunk(&mut png, b"tEXt", b"Software\0gnome-screenshot");
    expected(
        "Screenshot_20250407-123123.png",
        png,
        Kind::Photos,
        &["camera_exif"],
    );
}

#[test]
fn utf16_and_legacy_subrip_are_decoded() {
    let utf16: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(VTT.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    expected("captions.vtt", utf16, Kind::Transcripts, &["webvtt_cues"]);
    let legacy = SRT
        .as_bytes()
        .iter()
        .copied()
        .map(|c| if c == b'W' { 0x93 } else { c })
        .collect::<Vec<_>>();
    expected("captions.srt", legacy, Kind::Transcripts, &["subrip_cues"]);
}

fn expect_review_file(name: &str, text: &str, kind: Kind, reason: &str) {
    let found = classify(text.as_bytes(), name, Path::new("inbox"), None, None);
    assert_eq!(found.kind, kind, "{name}: {:?}", found.reasons);
    assert_eq!(found.reasons, [reason], "{name}");
}

#[test]
fn weekday_habit_log() {
    expect_review_file(
        "habits.txt",
        "Mon: ran 3 km, slept fine
Tue: ran 3 km, slept fine
Wed: ran 3 km, slept fine
Thu: ran 3 km, slept fine
Fri: ran 3 km, slept fine
Sat: ran 3 km, slept fine
Sun: ran 3 km, slept fine
Mon: ran 4 km, slept fine
Tue: ran 4 km, slept fine
Wed: ran 4 km, slept fine
Thu: ran 4 km, slept fine
Fri: ran 4 km, slept fine
Sat: ran 4 km, slept fine
Sun: ran 4 km, slept fine
Mon: ran 5 km, slept fine
Tue: ran 5 km, slept fine
Wed: ran 5 km, slept fine
Thu: ran 5 km, slept fine
Fri: ran 5 km, slept fine
Sat: ran 5 km, slept fine
Sun: ran 5 km, slept fine
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn meal_diary() {
    expect_review_file(
        "food.md",
        "Breakfast: oats and coffee
Lunch: soup with bread
Dinner: rice and beans
Breakfast: oats and coffee
Lunch: soup with bread
Dinner: rice and beans
Breakfast: oats and coffee
Lunch: soup with bread
Dinner: rice and beans
Breakfast: oats and coffee
Lunch: soup with bread
Dinner: rice and beans
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn repeated_settings() {
    expect_review_file(
        "settings.txt",
        "host: alpha
port: 8080
mode: fast
host: beta
port: 8081
mode: slow
host: gamma
port: 8082
mode: slow
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn round_robin_action_list() {
    expect_review_file(
        "actions.txt",
        "Dana: book the room
Priya: send the draft
Omar: check the budget
Dana: confirm catering
Priya: update the wiki
Omar: call the venue
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn poem_with_refrain() {
    expect_review_file(
        "poem.txt",
        "Tide: come in
Moon: pull hard
Tide: go out
Moon: let go
Tide: come in
Moon: pull hard
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn timed_run_log() {
    expect_review_file(
        "run.log",
        "09:00 - start: warm up done
09:10 - lap: first lap done
09:20 - lap: second lap done
09:30 - lap: third lap done
09:40 - stop: cool down now
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn interview_with_title() {
    expect_review_file(
        "interview.txt",
        "Interview with Priya

Dana: question 0 here?
Priya: answer 0 follows.
Dana: question 1 here?
Priya: answer 1 follows.
Dana: question 2 here?
Priya: answer 2 follows.
Dana: question 3 here?
Priya: answer 3 follows.
Dana: question 4 here?
Priya: answer 4 follows.
",
        Kind::Transcripts,
        "bare_speaker",
    );
}

#[test]
fn plan_with_act_in_a_sentence() {
    expect_review_file(
        "plan.md",
        "Act 2 of the plan starts in March.

TODO
book venue

IDEAS
ask Omar
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn paint_note_with_scene_like_lines() {
    expect_review_file(
        "paint.txt",
        "INT. walls need two coats
EXT. trim is fine

BUDGET
about 400

NOTES
ask for samples
",
        Kind::Notes,
        "note_default",
    );
}

#[test]
fn bold_weekday_habit_log() {
    let text = "**Mon:** walked two miles\n**Wed:** rode to work\n**Fri:** swam before lunch\n**Mon:** took the bus\n**Wed:** walked home\n**Fri:** rode to the park\n";
    expect_review_file("habit.md", text, Kind::Notes, "note_default");
}

#[test]
fn bracketed_weekday_habit_log() {
    let text = "[06:20] Mon: walked two miles\n[06:40] Wed: rode to work\n[07:15] Fri: swam before lunch\n[07:45] Mon: took the bus\n[08:20] Wed: walked home\n[08:50] Fri: rode to the park\n";
    expect_review_file("habit.log", text, Kind::Notes, "note_default");
}

#[test]
fn script_character_names_may_also_be_field_labels() {
    expect_review_file(
        "play.txt",
        "INT. RESTAURANT - AFTERNOON\n\nMAY\n(quietly)\nHow much time do we have?\n\nCOOK\nThe bread is almost ready.\n",
        Kind::Documents,
        "script_scenes",
    );
}

#[test]
fn a_role_export_can_keep_its_full_markdown_title() {
    expect_review_file(
        "conversation.md",
        "# A conversation about preparing the school garden for the first week of winter\n\n#### You:\nDo we still have burlap?\n\n#### ChatGPT:\nA tarp would work too.\n",
        Kind::Transcripts,
        "role_headers",
    );
}

#[test]
fn generic_turn_and_script_boundary_samples() {
    let bare = "Ari: Shall we use brass?\nBo: It might tarnish.\nAri: Could we seal it?\nBo: I can bring sealant.\nAri: I'll bring a brush.\nBo: See you there.\n";
    let cases: &[(&str, &str, Kind, &str)] = &[
        (
            "dated-chat.log",
            "2026-04-17\n\n[08:10] Ari: Should I move it?\n[08:12] Bo: Leave it there.\n[08:13] Ari: I'll leave a sign.\n",
            Kind::Transcripts,
            "bracketed_speaker",
        ),
        (
            "full-clock-habits.log",
            "[06:20:00] Mon: walked to work\n[07:05:00] Tue: took the bus\n[07:40:00] Wed: rode to work\n[08:10:00] Mon: walked home\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "full-clock-dialogue.log",
            "[06:20:00] May: Is the kiln hot?\n[06:22:00] Ari: It needs ten minutes.\n[06:24:00] May: I'll wait outside.\n",
            Kind::Transcripts,
            "existing_timed_layout",
        ),
        (
            "calendar.md",
            "January: found the trail\nFebruary: took the long route\nMarch: closed for repairs\nJanuary: walked the ridge\nFebruary: explored the bend\nMarch: went to the park\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "alternating-metadata.txt",
            "Host: alpha\nPort: 8000\nHost: beta\nPort: 8001\nHost: gamma\nPort: 8002\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "timed-refrain.log",
            "08:00 - Oak: stay\n08:01 - Reed: bend\n08:02 - Oak: stay\n08:03 - Reed: bend\n08:04 - Oak: stay\n08:05 - Reed: bend\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "parenthesized-rounds.txt",
            "Ari (0:01): open the shed\nBo (0:02): move a stool\nCy (0:03): find a key\nAri (0:04): dry the shelf\nBo (0:05): paint the door\nCy (0:06): sweep the floor\nAri (0:07): oil the hinge\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "three-person-chat.txt",
            "Ari: Is the ladder ready?\nBo: It is outside.\nCy: I can bring it in.\nAri: I'll open the door.\nBo: Please bring a mat.\nCy: It's wet out there.\nBo: Take the old mat.\nAri: Here it is.\n",
            Kind::Transcripts,
            "bare_speaker",
        ),
        (
            "stacked-tasks.txt",
            "Ari 0:01\nOpen the shed\n\nBo 0:02\nMove a stool\n\nCy 0:03\nFind a key\n\nAri 0:04\nDry the shelf\n\nBo 0:05\nPaint the door\n\nCy 0:06\nSweep the floor\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "stacked-empty-body.txt",
            "Ari 0:01\n\nBo 0:02\nI have the keys.\n\nAri 0:03\nHere is the door.\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "play.txt",
            "Act II: The workshop\n\nARI.\nThe door needs oil.\n\nBO.\nI'll find a brush.\n",
            Kind::Documents,
            "script_scenes",
        ),
        (
            "budget.txt",
            "ACT II\n\nBUDGET\nPut aside thirty dollars.\n\nNOTES\nAsk for swatches.\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "one-script-cue.txt",
            "ACT II\n\nINT. CLOCK SHOP - NIGHT\nA pendant swings in the window.\n\nARI\nTime to close.\n",
            Kind::Notes,
            "note_default",
        ),
        (
            "whatsapp-field-names.txt",
            "[1/8/26, 08:04] May: Has the bell rung?\n[1/8/26, 08:05] June: Not yet.\n[1/8/26, 08:06] May: We'll wait.\n",
            Kind::Transcripts,
            "whatsapp_bracketed",
        ),
    ];
    for &(name, text, kind, reason) in cases {
        expect_review_file(name, text, kind, reason);
    }
    for (name, text, kind, reason) in [
        (
            "titled-chat.txt",
            format!("Brass workshop\n\n{bare}"),
            Kind::Transcripts,
            "bare_speaker",
        ),
        (
            "chat-in-notebook.md",
            format!(
                "# Brass workshop\n\n{bare}\n## Takeaways\nI am keeping the earlier exchange here alongside my notes about the tools.\n"
            ),
            Kind::Notes,
            "note_default",
        ),
        (
            "chat-with-short-prose-intro.txt",
            format!("I pasted the chat below\n\n{bare}"),
            Kind::Notes,
            "note_default",
        ),
    ] {
        expect_review_file(name, &text, kind, reason);
    }
}

#[test]
fn export_headers_and_dominant_turns_keep_the_transcript() {
    let header = "# Dock meeting\n**Date:** Oct 6 | **Duration:** 8m | **Participants:** Eleni, Lian\n## Overview\n";
    let turns = "\n## Transcript\n**[00:01] Eleni:** The parcel is waiting by the ramp.\n**[00:04] Lian:** Could we move it inside before the rain?\n**[00:07] Eleni:** I will ask June to unlock the shed.\n";
    assert_eq!(
        classify(
            format!("{header}Parcel delivery.{turns}").as_bytes(),
            "entry.md",
            Path::new("inbox"),
            None,
            None
        )
        .reasons[0],
        "meeting_export_turns"
    );
    expected(
        "entry.md",
        format!(
            "{header}### Route changes\n{}{turns}",
            "The delivery route has been reviewed by the committee. ".repeat(180)
        ),
        Kind::Notes,
        &["meeting_summary_note"],
    );
    expected(
        "entry.md",
        format!(
            "# Dock meeting\nDate: Oct 6\nI pasted only the exchange that went into this note.\n{turns}"
        ),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "entry.txt",
        "=== TRANSCRIPCIÓN DE CONVERSACIÓN ===\n[00:01] Tana: El barco sale al mediodía.\n[00:04] Ivo: Tengo los boletos.\n[00:07] Tana: Vamos al muelle.\n",
        Kind::Transcripts,
        &["meeting_export_turns", "bracketed_speaker"],
    );
    expected(
        "turns.txt",
        "Team report\n2026-10-06\nParticipants: Lian, Eleni\nLian: Is the kit dry?\nEleni: It needs another hour.\nLian: I'll open the window.\nEleni: Thanks for checking.\nLian: Is the spare ready?\nEleni: Yes, it is in the hall.\n",
        Kind::Transcripts,
        &["meeting_export_turns", "bare_speaker"],
    );
}

#[test]
fn a_meeting_header_does_not_turn_a_note_with_pasted_chat_into_a_transcript() {
    const NOTE: &str = "# Parts sync

Date: 2025-03-04
Attendees: Lena, Hugo

My own notes first: the supplier has been late three times this quarter and I think we should look for another one before summer.

Lena: did the parts arrive?
Hugo: two boxes came, one is missing.
Lena: which one is it?
Hugo: the bearings, I checked the slip.
Lena: can you call them today?
Hugo: yes, right after lunch.
";
    const PARAGRAPH: &str = "My own notes first: the supplier has been late three times this quarter and I think we should look for another one before summer.\n\n";

    expected("n.md", NOTE, Kind::Notes, &["note_default"]);
    expected(
        "n.md",
        NOTE.replace("My own notes first: ", ""),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "n.md",
        NOTE.replace("My own notes first:", "Note to self:"),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "n.md",
        NOTE.replace(
            "late three times this quarter and I think",
            "late three times this quarter.\nI think",
        )
        .replace("My own notes first:", "Supplier concerns:"),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "n.md",
        NOTE.replace(PARAGRAPH, ""),
        Kind::Transcripts,
        &["meeting_export_turns", "bare_speaker"],
    );
    expected(
        "n.md",
        NOTE.replace(PARAGRAPH, "Lena: I asked for the delivery times.\n\n"),
        Kind::Transcripts,
        &["meeting_export_turns", "bare_speaker"],
    );
}

#[test]
fn grammatical_annotations_do_not_count_as_speaker_names() {
    let glossary = "Kiln (noun): an oven for clay
Slip (noun): liquid clay for joining
Kiln (verb): to fire in an oven
Slip (verb): to coat with liquid clay
Glaze (noun): a glass coat
Glaze (verb): to apply that coat
";
    let meeting = "Lena Ortiz (she/her): did the parts arrive?
Hugo Bell (Bell Works): two boxes came, one is missing.
Lena Ortiz (she/her): which one is it?
Hugo Bell (Bell Works): the bearings, I checked the slip.
Lena Ortiz (she/her): can you call them today?
Hugo Bell (Bell Works): yes, right after lunch.
";
    expected("g.txt", glossary, Kind::Notes, &["note_default"]);
    expected("turns.txt", meeting, Kind::Transcripts, &["bare_speaker"]);
}

#[test]
fn labelled_exports_admit_annotations_emoji_and_exporter_namespaces() {
    for (text, rule) in [
        (
            "11:03:20\t From Bela (she/her) : Did the ferry leave?\n11:03:42\t From Oren (Dock Co) : It leaves at noon.\n",
            "zoom_chat",
        ),
        (
            "19:14  * pilot calls over\n19:15 <@fen> Are you on the pier?\n19:16 <+tei> I'm at the far end.\n19:17 <@fen> I see you now.\n",
            "irc_angle_nick",
        ),
        (
            "> <external>:U123 [U123] @ 09/03/2025 18:00:00 Z:\nReady?\n| > Neri [U456] @ 09/03/2025 18:00:24 Z:\n| Yes.\n",
            "slack_text",
        ),
        (
            "2026/10/04, 12:04 - 🤓: We arrived.\n2026/10/04, 12:05 - Bela: I see your bag.\n",
            "whatsapp_dash",
        ),
    ] {
        assert_eq!(
            classify(text.as_bytes(), "entry.txt", Path::new("inbox"), None, None).reasons[0],
            rule
        );
    }
}

#[test]
fn one_speaker_needs_an_exporter_mark() {
    let whatsapp = "[4/10/2569 BE, 9:01:22] Neri: The ferry is late.\n[4/10/2569 BE, 9:02:10] Neri added you\n[4/10/2569 BE, 9:04:02] Neri: We just boarded.\n";
    let notice =
        "[4/10/2569 BE, 8:00:00] Messages and calls are end-to-end encrypted. Tap to learn more.\n";
    expected(
        "entry.txt",
        format!("{notice}{whatsapp}"),
        Kind::Transcripts,
        &["whatsapp_bracketed"],
    );
    expected("entry.txt", whatsapp, Kind::Notes, &["note_default"]);
    expected("entry.txt", notice, Kind::Notes, &["note_default"]);

    let dutch = "01-10-2026 7:05 - Berichten en oproepen worden end-to-end versleuteld. Tik voor meer informatie.\n01-10-2026 7:06 - Sen: Goedemorgen.\n01-10-2026 7:07 - Sen: Tot straks.\n";
    expected("entry.txt", dutch, Kind::Transcripts, &["whatsapp_dash"]);
    let localized = "١٨\u{200f}/٣\u{200f}/٢٠٢٢، ٢:٥٦:٠٠ م";
    let arabic = format!(
        "[{localized}] الرسائل والمكالمات مشفرة تمامًا بين الطرفين.\n[{localized}] Sen: Ready.\n[{localized}] Sen: Let's leave.\n"
    );
    expected(
        "entry.txt",
        arabic,
        Kind::Transcripts,
        &["whatsapp_bracketed"],
    );

    let stacked = "Unknown Speaker  0:04\nThe wind is picking up.\nUnknown Speaker  2:13\nThe smaller boats have tied up.\n";
    let footer = "Transcribed by https://otter.ai\n";
    expected(
        "entry.txt",
        format!("{stacked}{footer}"),
        Kind::Transcripts,
        &["otter_export"],
    );
    expected("entry.txt", stacked, Kind::Notes, &["note_default"]);
    expected(
        "entry.txt",
        format!("I meant to find this later.\n{footer}"),
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "entry.txt",
        "Speaker 1 (00:06):\nI brought a rope for the dinghy.\nSpeaker 1 (01:26):\nIt is tied where the ramp meets the pier.\n",
        Kind::Transcripts,
        &["rev_numbered_speaker_clock"],
    );
    expected(
        "entry.txt",
        "Bela (00:06):\nI brought a rope for the dinghy.\nBela (01:26):\nIt is tied where the ramp meets the pier.\n",
        Kind::Notes,
        &["note_default"],
    );
    expected(
        "entry.txt",
        "[9:01] Bela: A strong wind.\n[9:02] Bela: One lantern out.\n[9:03] Bela: Lights come back.\n[9:04] Bela: The door shuts.\n[9:05] Bela: Clear skies now.\n",
        Kind::Notes,
        &["note_default"],
    );
}

#[test]
fn markdown_chat_exporter_signatures_require_both_roles() {
    let claude = "# Ferry times\n\n# Human — May 18, 2026, 10:13 AM\nWhen is the next ferry?\n# Claude — May 18, 2026, 10:14 AM\nAt noon.\n";
    expected(
        "entry.md",
        claude,
        Kind::Transcripts,
        &["ai_chat_markdown_export"],
    );
    expected(
        "entry.md",
        format!("# My notes\nI kept the exchange for my own reference.\n{claude}"),
        Kind::Notes,
        &["note_default"],
    );
    let chatgpt = "# Ferry times\n#### You:\nWhen is the next ferry?\n#### ChatGPT:\nAt noon.\n#### You:\n#### ChatGPT:\nThe next one after that is at three.\n";
    expected(
        "entry.md",
        chatgpt,
        Kind::Transcripts,
        &["ai_chat_markdown_export"],
    );
    let exporter = "---\nauthor: chatgpt\ncount: 2\nexporter: 3.0.0\n---\n# Ferry times\n## chat-1\n> When is the next ferry?\nAt noon.\n## chat-2\n> And the next one?\nAt three.\n";
    expected(
        "entry.md",
        exporter,
        Kind::Transcripts,
        &["ai_chat_markdown_export"],
    );
    expected(
        "entry.md",
        exporter.split("## chat-2").next().unwrap(),
        Kind::Notes,
        &["note_default"],
    );
}

#[test]
fn bare_flashcards_stay_notes_and_identified_interviews_keep_their_kind() {
    for deck in [
        "Front: Ancient clock\nBack: A sundial\nFront: Ocean clock\nBack: A tide gauge\nFront: Kitchen clock\nBack: A timer\n",
        "# Navigation\nQ: When does the ferry leave?\nA: At noon.\nQ: Where is the rope?\nA: In the shed.\nQ: Who has the key?\nA: The harbor master.\n",
    ] {
        expected("entry.md", deck, Kind::Notes, &["note_default"]);
    }
    let interview = "# Interview with Eleni\nQ: When did you start?\nA: In May.\nQ: Who had the key?\nA: Lian had it.\n";
    expected("entry.md", interview, Kind::Transcripts, &["role_labels"]);
}

fn add_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], value: &[u8]) {
    png.extend(u32::try_from(value.len()).unwrap().to_be_bytes());
    png.extend(kind);
    png.extend(value);
    // The minimal PNGs in these tests exercise metadata only.
    png.extend([0; 4]);
}

#[test]
fn screenshot_metadata_is_field_specific() {
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:e="http://ns.adobe.com/exif/1.0/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:RDF><rdf:Description><e:UserComment><rdf:Alt><rdf:li xml:lang="x-default">Screenshot</rdf:li></rdf:Alt></e:UserComment></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let png = |kind, data: &[u8]| {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        add_png_chunk(&mut bytes, kind, data);
        bytes
    };
    let xmp_png = |xml: &str| {
        let data = [b"XML:com.adobe.xmp\0\0\0\0\0".as_slice(), xml.as_bytes()].concat();
        png(b"iTXt", &data)
    };
    expected(
        "neutral.png",
        xmp_png(xmp),
        Kind::Screen,
        &["screenshot_user_comment"],
    );
    expected(
        "neutral.png",
        xmp_png(&xmp.replace("Screenshot", " Screenshot ")),
        Kind::Photos,
        &["image_default"],
    );

    let jpeg_segment = [b"http://ns.adobe.com/xap/1.0/\0".as_slice(), xmp.as_bytes()].concat();
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend(u16::try_from(jpeg_segment.len() + 2).unwrap().to_be_bytes());
    jpeg.extend(jpeg_segment);
    jpeg.extend([0xff, 0xd9]);
    expected(
        "neutral.jpg",
        jpeg,
        Kind::Screen,
        &["screenshot_user_comment"],
    );

    let mut webp = b"RIFF".to_vec();
    webp.extend(
        u32::try_from(20 + xmp.len() + xmp.len() % 2)
            .unwrap()
            .to_le_bytes(),
    );
    webp.extend(b"WEBPVP8 \0\0\0\0XMP ");
    webp.extend(u32::try_from(xmp.len()).unwrap().to_le_bytes());
    webp.extend(xmp.as_bytes());
    webp.resize(webp.len() + xmp.len() % 2, 0);
    expected(
        "neutral.webp",
        webp,
        Kind::Screen,
        &["screenshot_user_comment"],
    );

    expected(
        "neutral.png",
        xmp_png(&xmp.replace("Screenshot", "Screenshot copy")),
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "neutral.png",
        xmp_png(&xmp.replace("UserComment", "Description")),
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "neutral.png",
        xmp_png(&xmp.replace("http://ns.adobe.com/exif/1.0/", "urn:unrelated")),
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "neutral.png",
        png(b"tEXt", b"Software\0gnome-screenshot"),
        Kind::Screen,
        &["gnome_screenshot_software"],
    );
    expected(
        "neutral.png",
        png(b"iTXt", b"Software\0\0\0\0\0gnome-screenshot"),
        Kind::Screen,
        &["gnome_screenshot_software"],
    );
    expected(
        "neutral.png",
        png(b"tEXt", b"Description\0gnome-screenshot"),
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "neutral.png",
        png(b"tEXt", b"Software\0gnome-screenshot copy"),
        Kind::Photos,
        &["image_default"],
    );
    let mut truncated = png(b"tEXt", b"Software\0gnome-screenshot");
    truncated.pop();
    expected("neutral.png", truncated, Kind::Photos, &["image_default"]);

    let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
    tiff.extend(b"\x69\x87\x04\0\x01\0\0\0\x1a\0\0\0\0\0\0\0");
    tiff.extend(b"\x01\0\x86\x92\x07\0\x12\0\0\0\x2c\0\0\0\0\0\0\0");
    tiff.extend(b"ASCII\0\0\0Screenshot");
    expected(
        "neutral.png",
        png(b"eXIf", &tiff),
        Kind::Screen,
        &["screenshot_user_comment"],
    );

    let clean_android = png(b"tEXt", b"Software\0Android QQ7A.8");
    expected(
        "neutral.png",
        &clean_android,
        Kind::Screen,
        &["android_screenshot_software"],
    );
    expected(
        "neutral.png",
        png(b"tEXt", b"Software\0Android "),
        Kind::Photos,
        &["image_default"],
    );
    let mut camera_make_only = clean_android;
    add_png_chunk(
        &mut camera_make_only,
        b"eXIf",
        b"II*\0\x08\0\0\0\x01\0\x0f\x01\x02\0\x04\0\0\0ABC\0\0\0\0\0",
    );
    expected(
        "neutral.png",
        camera_make_only,
        Kind::Photos,
        &["image_default"],
    );
}
