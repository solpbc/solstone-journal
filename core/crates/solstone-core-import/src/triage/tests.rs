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
    assert_eq!(found.ruleset_version, 1);
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
        assert!(!rule.verified_on_real_file);
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
            "Q: Have you met?\nA: At the station.\nQ: When was that?\n",
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
