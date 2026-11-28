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
    assert_eq!(found.ruleset_version, RULESET_VERSION);
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

    // Minimal uncompressed stored-ZIP helper so triage tests cover XLSB and
    // negative non-Office ZIPs without extra dev-dependencies.
    fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = local.len() as u32;
            let name_bytes = name.as_bytes();
            let size = data.len() as u32;
            // Local file header
            local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes()); // version needed
            local.extend_from_slice(&0u16.to_le_bytes()); // flags
            local.extend_from_slice(&0u16.to_le_bytes()); // method: stored
            local.extend_from_slice(&0u16.to_le_bytes()); // mod time
            local.extend_from_slice(&0u16.to_le_bytes()); // mod date
            local.extend_from_slice(&0u32.to_le_bytes()); // crc32 (unused by name scan)
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name_bytes);
            local.extend_from_slice(data);

            // Central directory header
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name_bytes);
        }
        let cd_offset = local.len() as u32;
        let cd_size = central.len() as u32;
        let count = entries.len() as u16;
        let mut out = local;
        out.extend_from_slice(&central);
        // EOCD
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    // XLSB (xl/workbook.bin) and encrypted OOXML CFB classify as Documents unhinted.
    let xlsb = stored_zip(&[("xl/workbook.bin", b"\x00\x01\x02")]);
    expected(
        "workbook.xlsb",
        &xlsb,
        Kind::Documents,
        &["document_format"],
    );

    // Minimal MS-CFB v3 holding EncryptionInfo + EncryptedPackage.
    let mut enc_cfb = vec![0u8; 512 * 3];
    enc_cfb[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    enc_cfb[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    enc_cfb[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    enc_cfb[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    enc_cfb[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    enc_cfb[0x44..0x48].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    enc_cfb[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    for i in 1..109 {
        enc_cfb[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    enc_cfb[512..516].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
    enc_cfb[516..520].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    for i in 2..128 {
        enc_cfb[512 + i * 4..512 + i * 4 + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    let write_entry = |buf: &mut [u8], name: &str, kind: u8, right: u32, child: u32| {
        let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        for (i, u) in units.iter().enumerate() {
            buf[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        buf[0x40..0x42].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
        buf[0x42] = kind;
        buf[0x44..0x48].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        buf[0x48..0x4C].copy_from_slice(&right.to_le_bytes());
        buf[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
    };
    write_entry(&mut enc_cfb[1024..1152], "Root Entry", 5, 0xFFFF_FFFF, 1);
    write_entry(
        &mut enc_cfb[1152..1280],
        "EncryptionInfo",
        2,
        2,
        0xFFFF_FFFF,
    );
    write_entry(
        &mut enc_cfb[1280..1408],
        "EncryptedPackage",
        2,
        0xFFFF_FFFF,
        0xFFFF_FFFF,
    );
    expected(
        "secret.docx",
        &enc_cfb,
        Kind::Documents,
        &["document_format"],
    );

    // Negative twin: non-Office ZIP must NOT classify as Documents.
    let plain_zip = stored_zip(&[("conversations.json", b"[]")]);
    let found = verdict("export.zip", &plain_zip);
    assert_ne!(found.kind, Kind::Documents);
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
        "Capture 2025-04-07 123123.png",
        &png,
        Kind::Photos,
        &["camera_exif"],
    );
    add_png_chunk(&mut png, b"tEXt", b"Software\0Greenshot");
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
    for annotation in ["adj", "adv"] {
        let abbreviated = format!(
            "Fast ({annotation}): quick\nLate ({annotation}): after the expected time\n\
             Fast ({annotation}): at speed\nLate ({annotation}): behind time\n\
             Fast ({annotation}): with little delay\nLate ({annotation}): well into the night\n"
        );
        expected("g.txt", abbreviated, Kind::Notes, &["note_default"]);
    }
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

#[test]
fn video_chapters_and_leading_time_notes_stay_notes() {
    let bare_chapters = "Greenhouse tour\n\n0:00 Intro\n1:23 Pricing\n4:10 Demo\n7:45 Q&A\n";
    let bracketed_chapters = "# Greenhouse tour\n\n[00:00:00] Intro\n[00:01:23] Pricing\n[00:04:10] Demo\n[00:07:45] Q&A\n";
    let transcript_titled_chapters = "# Greenhouse tour — Transcript\n\n[00:00:00] Intro and overview\n[00:01:23] Pricing tiers and annual billing\n[00:04:10] Live demo of the new workflow\n[00:07:45] Questions from the audience\n";
    let sentence_chapters = "# Greenhouse tour — Transcript\n\n[00:00:00] Welcome to the show and a brief look at what we will cover today.\n[00:01:23] Walking through the updated pricing tiers and how annual discounts work.\n[00:04:10] A short walkthrough of the import flow and what changed this month.\n[00:07:45] Answering viewer questions about exports and closing out the episode.\n";
    let bulleted_chapters = "# Greenhouse tour — Transcript\n\n- [00:00:00] The cedar greenhouse on Ridge Road opened its glass roof after three winters under canvas tarps, and volunteers replaced every brass latch.\n- [00:01:00] The municipal water line still needs a pressure valve inspection, and the county clerk said the paperwork takes six weeks.\n- [00:02:00] Meanwhile the head botanist fills watering cans from the rainwater cistern whenever warm winds dry out the fern beds.\n";
    let editing_notes = "# Greenhouse tour — Transcript notes\n\n[00:00:00] Trim the opening silence and check spelling of Ridge Road in the opening narration paragraph before publishing the final audio cut.\n[00:01:00] Fix the slight audio dip right before the valve section and lower the sprinkler background noise across the whole middle passage.\n[00:02:00] Verify whether the botanist still waters from the west cistern and add credits before the fade out.\n";
    let leading_times_narration = "Workshop Notes: the pottery kiln\n\n0:00 Welcome to Workshop Notes. Today we inspect the brick kiln behind the east studio.\n0:14 - It was built in 1924 to fire stoneware pitchers through autumn markets.\n0:31: The firebrick arch is intact and still carries bands of wood ash.\n1:45 Today the guild stores glaze buckets and spare pyrometer cones inside.\n2:10\nNobody remembers which mason signed the keystone above the door.\n";
    let leading_times_edit_notes = "Workshop Notes: rough cut edits\n\n0:00 Trim two seconds off the opening silence before the music comes in.\n0:14 - Color grade is too warm on the exterior shot of the brick arch.\n0:31: Lower the street traffic hum so the narration stays clear.\n1:45 Check whether we have permission to show the glaze shelf close-up.\n2:10\nAdd lower-third credits before fading to black.\n";
    let prose_before_timed = "# Greenhouse tour — Transcript\n\nI saved this clip because I want to verify the dates before writing the article.\n\n[00:00:00] The cedar greenhouse on Ridge Road opened its glass roof after three winters under canvas tarps, and volunteers replaced every brass latch.\n\nThe timber frame spans forty feet and the [00:01:00] municipal water line still needs a valve inspection that takes six weeks to schedule.\n\nMeanwhile the head botanist [00:02:00] fills watering cans from the rainwater cistern when warm winds dry out the fern beds.\n";
    let trailing_section = "# Greenhouse tour — Transcript\n\n[00:00:00] The cedar greenhouse on Ridge Road opened its glass roof after three winters under canvas tarps, and volunteers replaced every brass latch.\n\nThe timber frame spans forty feet and the [00:01:00] municipal water line still needs a valve inspection that takes six weeks to schedule.\n\nMeanwhile the head botanist [00:02:00] fills watering cans from the rainwater cistern when warm winds dry out the fern beds.\n\n## My takeaways\n\nFollow up with the county clerk next Tuesday.\n";

    for (name, text) in [
        ("chapters.txt", bare_chapters),
        ("chapters.md", bracketed_chapters),
        ("titled-chapters.md", transcript_titled_chapters),
        ("sentence-chapters.md", sentence_chapters),
        ("bulleted-chapters.md", bulleted_chapters),
        ("editing-notes.md", editing_notes),
        ("leading-times.txt", leading_times_narration),
        ("edit-notes.txt", leading_times_edit_notes),
        ("prose-before.md", prose_before_timed),
        ("trailing-notes.md", trailing_section),
    ] {
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn near_misses_for_transcript_expansions_stay_notes() {
    let stacked_turns = "Nora  0:03\nDid the ferry dock on time?\n\nTarek  0:09\nFive minutes early because the tide helped.\n\nNora  0:18\nGood, then we can unload before noon.\n";
    for (name, text) in [
        ("note-prefix.txt", format!("Note: pasted from today\n\n{stacked_turns}")),
        ("note-to-self.txt", format!("Note to self: check the schedule\n\n{stacked_turns}")),
        ("my-notes.txt", format!("My notes: ferry call excerpt\n\n{stacked_turns}")),
        ("meeting-notes.txt", format!("Meeting notes: harbor sync\n\n{stacked_turns}")),
        ("summary-prefix.txt", format!("Summary: ferry arrival times\n\n{stacked_turns}")),
        ("reminder-prefix.txt", format!("Reminder: call Tarek before noon\n\n{stacked_turns}")),
        (
            "range-schedule.txt",
            "[00:00:00 - 00:05:00] Opening remarks\n[00:05:00 - 00:10:00] Live demo\n[00:10:00 - 00:15:00] Closing\n".to_string(),
        ),
        (
            "clock-schedule.txt",
            "[09:00 - 09:30] Design: review wireframes\n[09:30 - 10:00] Backend: inspect routes\n[10:00 - 10:30] Design: revise cards\n[10:30 - 11:00] Backend: run checks\n".to_string(),
        ),
        (
            "range-log.txt",
            "[00:00:00 - 00:00:05] INFO: booting worker\n[00:00:05 - 00:00:10] WARN: retrying socket\n[00:00:10 - 00:00:15] INFO: worker ready\n".to_string(),
        ),
        (
            "range-one-speaker.txt",
            "[00:00:00 - 00:00:06] Nora: First observation about the kiln.\n[00:00:06 - 00:00:12] Nora: Second observation about the glaze.\n[00:00:12 - 00:00:18] Nora: Third observation about the shelf.\n".to_string(),
        ),
        (
            "dated-changelog.md",
            "- 2026-08-05 01:00:05 **Auth**: Rotated signing key.\n- 2026-08-05 01:00:18 **Billing**: Synced invoice batch.\n- 2026-08-05 01:00:34 **Search**: Rebuilt index shard.\n- 2026-08-05 01:00:49 **Cache**: Flushed stale entries.\n".to_string(),
        ),
        (
            "dated-log-levels.md",
            "- 2026-08-05 01:00:05 **INFO**: Starting worker.\n- 2026-08-05 01:00:18 **WARN**: Retrying socket.\n- 2026-08-05 01:00:34 **INFO**: Ready for work.\n".to_string(),
        ),
        (
            "stacked-roles.md",
            "# Standup\n\n**Nora**\n*Design lead*\nKeep the annual plan clear.\n\n**Tarek**\n*Operations*\nTuesday works for me.\n".to_string(),
        ),
        (
            "stacked-schedule.md",
            "# Conference day\n\n**Keynote**\n*09:00*\nOpening remarks in the main hall.\n\n**Workshop**\n*10:30*\nHands-on session in room four.\n".to_string(),
        ),
        (
            "stacked-three-sessions.md",
            "# Conference day\n\n**Keynote**\n*09:00*\nOpening remarks in the main hall.\n\n**Workshop**\n*10:30*\nHands-on session in room four.\n\n**Closing**\n*12:00*\nWrap-up in the courtyard.\n".to_string(),
        ),
        (
            "stacked-one-speaker.md",
            "# Notes\n\n**Nora**\n*00:00*\nFirst thought on the dock.\n\n**Nora**\n*00:04*\nSecond thought on the dock.\n".to_string(),
        ),
        (
            "stacked-prose-intro.md",
            "# Notes\n\nI saved this excerpt from the morning call.\n\n**Nora**\n*00:00*\nKeep the annual plan clear.\n\n**Tarek**\n*00:04*\nTuesday works for me.\n".to_string(),
        ),
        (
            "stacked-huge-clock.md",
            "# Standup\n\n**Nora**\n*99999999999999999:00*\nCheck the sensor.\n\n**Tarek**\n*+0:04*\nDone.\n".to_string(),
        ),
        (
            "daemon.log",
            "09:00:01 auth: issued token for worker\n09:00:02 cache: cold miss on key\n09:00:03 auth: refreshed session\n".to_string(),
        ),
        (
            "components.log",
            "09:00:01 Auth: issued token for worker\n09:00:02 Cache: cold miss on key\n09:00:03 Queue: flushed batch\n".to_string(),
        ),
        (
            "prose-before-meeting.txt",
            "This is a sample call excerpt I kept for reference.\n\nMeeting: Harbor kickoff - 2026-05-10 10:00 AM\n\n00:00:01 Speaker 1 (Nora): Good morning everyone.\n00:00:15 Speaker 2 (Tarek): Glad to be here.\n00:00:25 Speaker 1 (Nora): Let us review the piers.\n".to_string(),
        ),
        (
            "irc-prose-underline.log",
            "I copied this log from yesterday\n========\n00:23 < nora> Ready for the test?\n00:24 < tarek> Almost there.\n00:25 < nora> Starting now.\n".to_string(),
        ),
        (
            "weechat-notices-only.log",
            "2026-05-10 06:44:16\t◥\tnora has joined #dock\n2026-05-10 06:44:16\t--\tTopic for #dock is Harbor tools\n2026-05-10 06:48:33\t◣\ttarek has quit\n2026-05-10 07:00:00\tnora\tAnyone here?\n".to_string(),
        ),
        (
            "imessage-prose-intro.txt",
            "Notes from our group chat:\n\nMay 17, 2025  5:29:42 PM You named the conversation Harbor Crew\n\nMay 17, 2025  5:34:00 PM\nNora\nBringing rope.\n\nMay 17, 2025  5:36:00 PM\nTarek\nSee you at the dock.\n".to_string(),
        ),
        (
            "imessage-events-only.txt",
            "May 17, 2025  5:29:42 PM You named the conversation Harbor Crew\n\nMay 17, 2025  5:30:00 PM Tarek named the conversation Dock Crew\n\nMay 17, 2025  5:34:00 PM\nNora\nBringing rope.\n".to_string(),
        ),
        (
            "imessage-event-as-body.txt",
            "May 17, 2025  5:34:00 PM\nNora\nMay 17, 2025  5:35:00 PM Tarek named the conversation Dock Crew\n\nMay 17, 2025  5:36:00 PM\nTarek\nSee you at the dock.\n".to_string(),
        ),
        (
            "signal-prose-tail.md",
            "[2025-08-17 09:00:00] Nora: nice photo!\n[2025-08-17 09:01:00] Tarek: thanks!\nHere is my commentary on what we should do next.\nFirst we need to call the harbor office.\nThen we should inspect the mooring lines.\nAfter that we update the logbook.\nFinally we lock the boathouse.\n".to_string(),
        ),
        (
            "signal-huge-quote.md",
            "[2025-08-17 09:00:00] Nora: read this\n[2025-08-17 09:01:00] Tarek: reading\n> line one\n> line two\n> line three\n> line four\n> line five\n> line six\n> line seven\n".to_string(),
        ),
        (
            "whisper-owner-notes.md",
            "# Transcription: Harbor Episode 4\n\nSource: https://example.invalid/ep4\nTranscription model: whisper-small\n\nMachine-generated transcript. Review speaker labels before quoting.\n\nMy own notes on this talk: the second section has the permit dates we need.\n\n**[00:00] Nora:** Welcome back to the harbor log.\n**[00:08] Nora:** Today we inspect the north quay.\n**[00:16] Nora:** The stone wall held through winter.\n".to_string(),
        ),
        (
            "whisper-owner-notes-plain.md",
            "# Transcription: Harbor Episode 4\n\nSource: https://example.invalid/ep4\nTranscription model: whisper-small\n\nMachine-generated transcript. Review speaker labels before quoting.\n\nI copied this excerpt because the second section has the permit dates we need.\n\n**[00:00] Nora:** Welcome back to the harbor log.\n**[00:08] Nora:** Today we inspect the north quay.\n**[00:16] Nora:** The stone wall held through winter.\n".to_string(),
        ),
        (
            "single-speaker-no-model.md",
            "# Harbor Episode 4\n\nSource: https://example.invalid/ep4\nDate: 2026-06-01\n\n**[00:00] Nora:** Welcome back to the harbor log.\n**[00:08] Nora:** Today we inspect the north quay.\n**[00:16] Nora:** The stone wall held through winter.\n".to_string(),
        ),
        (
            "single-speaker-untimed.md",
            "# Transcription: Harbor Episode 4\n\nSource: https://example.invalid/ep4\nTranscription model: whisper-small\n\nNora: Welcome back to the harbor log.\nNora: Today we inspect the north quay.\nNora: The stone wall held through winter.\n".to_string(),
        ),
        (
            "vtt-with-prose.vtt",
            "WEBVTT\n\n00:01.000 --> 00:02.000\nWind rose.\n\n00:03.000 --> 00:04.000\nRain fell.\n\n00:05.000 --> 00:06.000\nTide turned.\n\n00:07.000 --> 00:08.000\nBoats swayed.\n\nHere are my own notes about how these captions looked.\n".to_string(),
        ),
        (
            "vtt-mostly-bad.vtt",
            "WEBVTT\n\n00:01.000 --> 00:02.000\nWind rose.\n\n00:03.000 --> 00:04.000\nRain fell.\n\n00:05.000 --> 00:0x.000\nbad cue\n".to_string(),
        ),
    ] {
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn real_two_person_conversations_survive_two_label_guards() {
    for (name, text, rule) in [
        (
            "short-names.txt",
            "Vi: Did the crate arrive?\nAn: Yes, by the back door.\nVi: Is the lid intact?\nAn: One corner is chipped.\nVi: I'll bring glue.\nAn: Thanks, see you soon.\n",
            "bare_speaker",
        ),
        (
            "initials.txt",
            "JM: Did the crate arrive?\nKL: Yes, by the back door.\nJM: Is the lid intact?\nKL: One corner is chipped.\nJM: I'll bring glue.\nKL: Thanks, see you soon.\n",
            "bare_speaker",
        ),
        (
            "mixed-initials.txt",
            "EN: Did the crate arrive?\nJM: Yes, by the back door.\nEN: Is the lid intact?\nJM: One corner is chipped.\nEN: I'll bring glue.\nJM: Thanks, see you soon.\n",
            "bare_speaker",
        ),
        (
            "timed-initials.log",
            "[09:01] EN: Did the crate arrive?\n[09:02] FR: Yes, by the back door.\n[09:03] EN: I'll bring glue.\n",
            "bracketed_speaker",
        ),
        (
            "numeric-answers.txt",
            "Nora: How many crates arrived?\nTarek: 12\nNora: How many had chipped lids?\nTarek: 3\nNora: What did the invoice come to?\nTarek: $240.50\n",
            "bare_speaker",
        ),
    ] {
        expected(name, text, Kind::Transcripts, &[rule]);
    }
}

#[test]
fn two_label_price_logs_and_translation_pairs_stay_notes() {
    let prices =
        "Gold: 2301.5\nSilver: 27.1\nGold: 2314.2\nSilver: 27.4\nGold: 2298.8\nSilver: 26.9\n";
    let currency_prices = "Gold: $2,301.50\nSilver: $27.10\nGold: $2,314.20\nSilver: $27.40\nGold: $2,298.80\nSilver: $26.90\n";
    let bold_prices = "**Gold:** 2301.5 USD\n**Silver:** 27.1 USD\n**Gold:** 2314.2 USD\n**Silver:** 27.4 USD\n**Gold:** 2298.8 USD\n**Silver:** 26.9 USD\n";
    let metrics = "Weight: 182.4 lb\nDistance: 5.2 km\nWeight: 181.9 lb\nDistance: 6.1 km\nWeight: 181.5 lb\nDistance: 4.8 km\n";
    let translations = "EN: Good morning, how are you?\nFR: Bonjour, comment allez-vous ?\nEN: Where is the harbor?\nFR: Où est le port ?\nEN: Thank you very much.\nFR: Merci beaucoup.\n";
    let lowercase_translations = "en: Good morning, how are you?\nfr: Bonjour, comment allez-vous ?\nen: Where is the harbor?\nfr: Où est le port ?\nen: Thank you very much.\nfr: Merci beaucoup.\n";
    let locale_translations = "en-US: Good morning, how are you?\nfr-FR: Bonjour, comment allez-vous ?\nen-US: Where is the harbor?\nfr-FR: Où est le port ?\nen-US: Thank you very much.\nfr-FR: Merci beaucoup.\n";
    let named_translations = "English: Good morning, how are you?\nFrench: Bonjour, comment allez-vous ?\nEnglish: Where is the harbor?\nFrench: Où est le port ?\nEnglish: Thank you very much.\nFrench: Merci beaucoup.\n";
    let bold_translations = "**EN:** Good morning, how are you?\n**FR:** Bonjour, comment allez-vous ?\n**EN:** Where is the harbor?\n**FR:** Où est le port ?\n**EN:** Thank you very much.\n**FR:** Merci beaucoup.\n";

    for (name, text) in [
        ("prices.txt", prices),
        ("currency.txt", currency_prices),
        ("prices.md", bold_prices),
        ("metrics.txt", metrics),
        ("translations.txt", translations),
        ("translations-lower.txt", lowercase_translations),
        ("translations-locale.txt", locale_translations),
        ("translations-named.txt", named_translations),
        ("translations.md", bold_translations),
    ] {
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn chat_and_subtitle_edge_layouts_classify_as_transcripts() {
    let vtt_with_bad_cue = concat!(
        "WEBVTT - Harbor captions\n",
        "Kind: captions\n\n",
        "NOTE\nCaption notes\n\n",
        "STYLE\n::cue { color: white; }\n\n",
        "part 1\n00:00:00.000 --> 00:00:02.000\nFirst line.\n\n",
        "00:00:03.000 --> 00:00:05.000\nSecond line.\n\n",
        "00:00:06.000 --> 00:00:08.000\nThird line.\n\n",
        "00:00.000 --> 00:0x.000\nbad cue\n\n",
        "00:00:09.000 --> 00:00:11.000\nFourth line.\n\n",
        "00:00:12.000 --> 00:00:14.000\nFifth line.\n",
    );
    let imessage_with_renames = concat!(
        "May 17, 2025  5:29:42 PM You named the conversation Harbor Crew\n\n",
        "May 17, 2025  5:34:00 PM\nNora\nBringing rope.\n\n",
        "May 17, 2025  5:35:00 PM Tarek named the conversation Dock Crew\n\n",
        "May 17, 2025  5:36:00 PM (Read by you after 1 minute, 4 seconds)\nTarek\nSee you at the dock.\n",
    );
    let signal_with_reactions = concat!(
        "[2025-08-17 09:00:00] Nora: nice photo!\n",
        "(- Tarek: 👍, Sora: ❤️ -)\n",
        "[2025-08-17 09:01:00] Tarek: line one\n",
        "line two\n",
        "(- Nora: 😂 -)\n",
        "[2025-08-17 09:02:00] Sora: > original\n",
        "> (- Milo -)\n\n",
        "reply\n",
        "[2025-08-17 09:03:00] Tarek: Look at the pier.\n",
        "(- Nora: 🔥 -)![pier.jpg](./media/pier.jpg)\n",
    );
    let weechat_unicode = concat!(
        "2026-05-10 06:44:16\t◥\tnora (~nora@host.invalid) has joined #dock\n",
        "2026-05-10 06:44:16\t--\tTopic for #dock is Harbor tools\n",
        "2026-05-10 06:48:33\t◣\tmilo (~milo@host.invalid) has quit\n",
        "2026-05-10 07:29:38\tnora\tDid the pump finish draining?\n",
        "2026-05-10 07:30:10\ttarek\tAlmost, give it five minutes.\n",
        "2026-05-10 07:31:02\tnora\tThanks, I will lock the shed after.\n",
    );
    let irc_setext = concat!(
        "Day 1\n",
        "========\n\n",
        "--- Day changed Fri May 10 2026\n",
        "00:23 < nora> Ready for the early tide?\n",
        "00:46 < tarek> Already on the pier.\n",
        "00:48 < nora> Bringing the lantern now.\n",
    );

    for (name, text, rule) in [
        ("captions.vtt", vtt_with_bad_cue, "webvtt_cues"),
        (
            "imessage-events.txt",
            imessage_with_renames,
            "dated_sender_blocks",
        ),
        ("signal-reactions.md", signal_with_reactions, "signal_dated"),
        ("weechat-unicode.log", weechat_unicode, "dated_tab_nick"),
        ("irc-setext.log", irc_setext, "irc_angle_nick"),
    ] {
        expected(name, text, Kind::Transcripts, &[rule]);
    }
}

#[test]
fn meeting_whisper_and_timed_narration_layouts_classify_as_transcripts() {
    let otter_colon_title = concat!(
        "Orchard Radio, episode 9: The cider press\n\n",
        "Sonia Cho  0:03\n",
        "Welcome back to the barn. Today we are looking at the oak cider press.\n\n",
        "Devon Cruz  0:12\n",
        "It was assembled in 1928 and only two screw threads needed re-cutting last autumn.\n\n",
        "Sonia Cho  0:25\n",
        "Did the millwright replace the ash frame?\n\n",
        "Devon Cruz  0:31\n",
        "No, only the brass washers under the hopper tray before harvest started.\n",
    );
    let whisperx_range = concat!(
        "[00:00:00 - 00:00:06] Speaker 1: Morning everyone, let us check the greenhouse vents.\n",
        "[00:00:06 - 00:00:13] Speaker 2: The north latch sticks slightly when humidity rises.\n",
        "[00:00:13 - 00:00:19] Speaker 1: Let us swap it with the spare hinge today.\n",
    );
    let dated_bold = concat!(
        "- 2026-08-05 01:00:05 **Speaker 1**: Let us get started with the workshop sync.\n",
        "- 2026-08-05 01:00:18 **Speaker 2**: The kiln thermocouple logs came back clean.\n",
        "- 2026-08-05 01:00:34 **Speaker 1**: Great, did the firing curve hold?\n",
        "- 2026-08-05 01:00:49 **Speaker 2**: Steady all week with no drops.\n",
    );
    let macwhisper_two_turn = concat!(
        "# Monday studio sync\n\n",
        "**Sonia**\n",
        "*00:00*\n",
        "Let us keep the glaze room open until seven.\n\n",
        "**Devon**\n",
        "*00:04*\n",
        "Seven works for me, I can lock the studio after.\n",
    );
    let macwhisper_three_turn = concat!(
        "# Studio sync\n\n",
        "**Sonia**\n",
        "*00:12*\n",
        "Did the stoneware clay arrive?\n\n",
        "**Devon**\n",
        "*00:28*\n",
        "Two pallets are sitting by the loading door.\n\n",
        "**Sonia**\n",
        "*00:45*\n",
        "I will wheel them into the damp room.\n",
    );
    let single_speaker_whisper = concat!(
        "# Transcription: Orchard Episode 19\n\n",
        "Source: https://example.invalid/ep19\n",
        "Media title: Orchard Episode 19 - Testing the frost alarm\n",
        "Upload date: 20260501\n",
        "Transcription model: mlx-whisper/whisper-small\n",
        "Generated at: 2026-06-01T12:00:00Z\n\n",
        "Machine-generated transcript. Review speaker labels and wording before\n",
        "using this as quote-grade source material.\n\n",
        "**[00:00] Sonia Cho:** Welcome back to the orchard engineering log.\n\n",
        "**[00:08] Sonia Cho:** Today we test the copper thermometer along the lower slope.\n\n",
        "**[00:17] Sonia Cho:** The relay tripped cleanly as soon as the breeze dropped below zero.\n",
    );
    let timed_narration_inline = concat!(
        "# The Cedar Greenhouse Reopens — Transcript (2026-11-12)\n\n",
        "https://example.invalid/e/12 · Listen: https://example.invalid/listen\n\n",
        "---\n\n",
        "[00:00:00] The cedar greenhouse on Ridge Road opened its glass roof after three winters under canvas tarps, and volunteers replaced every rusted latch.\n\n",
        "Last autumn the park trust hired a carpenter to rebuild the transom frames and seal the panes that cracked during an ice storm.\n\n",
        "The timber frame spans forty feet and the [00:01:00] municipal water line still waits on a pressure valve inspection that the clerk forgot to renew in time for spring.\n\n",
        "Meanwhile the head botanist [00:02:00] carries watering cans from the west rainwater cistern whenever dry afternoon winds sweep across the fern beds.\n",
    );
    let timed_narration_paragraphs = concat!(
        "# Ridge Road Dispatch — Transcript\n\n",
        "[00:00:00] The cedar greenhouse on Ridge Road opened its glass roof after three winters under canvas tarps, and volunteers spent all morning cleaning the overhead vents.\n\n",
        "[00:01:00] The municipal water line still waits on a pressure valve inspection that the county clerk forgot to renew before spring, and the office warned that review takes six weeks.\n\n",
        "[00:02:00] Meanwhile the head botanist carries watering cans from the west rainwater cistern whenever dry afternoon winds sweep across the raised fern beds.\n",
    );

    for (name, text, rule) in [
        ("otter-colon.txt", otter_colon_title, "stacked_speaker_time"),
        ("whisperx.txt", whisperx_range, "bracketed_speaker"),
        ("dated-bold.md", dated_bold, "dated_bold_speaker"),
        ("standup-two.md", macwhisper_two_turn, "macwhisper_stacked"),
        (
            "standup-three.md",
            macwhisper_three_turn,
            "macwhisper_stacked",
        ),
        (
            "whisper-solo.md",
            single_speaker_whisper,
            "meeting_export_turns",
        ),
        (
            "timed-inline.md",
            timed_narration_inline,
            "timed_transcript_narration",
        ),
        (
            "timed-blocks.md",
            timed_narration_paragraphs,
            "timed_transcript_narration",
        ),
    ] {
        expected(name, text, Kind::Transcripts, &[rule]);
    }
}

#[test]
fn schedules_changelogs_and_match_logs_stay_notes() {
    let plan = concat!(
        "Offsite plan: Denver in May\n\n",
        "Lena  9:00\n",
        "opens with the budget\n\n",
        "Hugo  9:30\n",
        "supplier review\n\n",
        "Lena  10:15\n",
        "wrap up\n",
    );
    let sched = concat!(
        "# Monday\n\n",
        "**Lena**\n",
        "*09:00*\n",
        "Budget review\n\n",
        "**Hugo**\n",
        "*09:30*\n",
        "Supplier call\n\n",
        "**Lena**\n",
        "*10:15*\n",
        "Wrap up\n",
    );
    let tasks = concat!(
        "09:00:00 Lena (lead): opens the shop\n",
        "09:30:00 Hugo (ops): checks deliveries\n",
        "10:00:00 Lena (lead): calls supplier\n",
        "10:30:00 Hugo (ops): packs orders\n",
    );
    let changelog = concat!(
        "- 2026-08-05 01:00:05 **Fixed**: the login loop on slow links.\n",
        "- 2026-08-06 01:00:18 **Added**: export to CSV.\n",
        "- 2026-08-07 01:00:34 **Fixed**: a crash on empty input.\n",
        "- 2026-08-08 01:00:49 **Added**: dark mode.\n",
    );
    let match_log = concat!(
        "Home: won 3-1 against Leeds\n",
        "Away: lost 0-2 at Hull\n",
        "Home: drew 1-1 with York\n",
        "Away: won 2-0 at Bath\n",
        "Home: won 4-0 against Hove\n",
        "Away: drew 0-0 at Ryde\n",
    );
    let plan_without_title = plan.split_once("\n\n").unwrap().1;
    let plan_title_over_recording = concat!(
        "Offsite plan: Denver in May\n\n",
        "Sonia Cho  0:03\n",
        "Welcome back to the studio.\n\n",
        "Devon Cruz  0:11\n",
        "Let us check the kiln log.\n\n",
        "Sonia Cho  0:19\n",
        "The glaze test finished overnight.\n",
    );

    for (name, text) in [
        ("plan.md", plan),
        ("sched.md", sched),
        ("t.txt", tasks),
        ("cl.md", changelog),
        ("s.txt", match_log),
        ("plan-body.md", plan_without_title),
        ("plan-title.md", plan_title_over_recording),
    ] {
        expect_review_file(name, text, Kind::Notes, "note_default");
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn additional_schedules_rosters_changelogs_and_logs_stay_notes() {
    let agenda_plain = concat!(
        "Friday agenda\n\n",
        "Lena  09:00\n",
        "Opens with the budget review.\n\n",
        "Hugo  09:30\n",
        "Walks through the supplier options.\n\n",
        "Lena  10:15\n",
        "Wraps up with next steps.\n",
    );
    let midnight_shifts = concat!(
        "# Night shift\n\n",
        "**Lena**\n",
        "*00:00*\n",
        "Server room patrol\n\n",
        "**Hugo**\n",
        "*00:30*\n",
        "Loading dock check\n",
    );
    let afternoon_agenda = concat!(
        "# Tuesday workshop\n\n",
        "**Lena**\n",
        "*13:00*\n",
        "Opens with the quarterly budget review.\n\n",
        "**Hugo**\n",
        "*13:30*\n",
        "Presents the updated supplier shortlist.\n\n",
        "**Lena**\n",
        "*14:15*\n",
        "Closes with action owners and dates.\n",
    );
    let range_roster = concat!(
        "[09:00:00 - 09:30:00] Lena: budget review\n",
        "[09:30:00 - 10:00:00] Hugo: supplier call\n",
        "[10:00:00 - 10:30:00] Lena: wrap up\n",
        "[10:30:00 - 11:00:00] Hugo: send notes\n",
    );
    let same_day_changelog = concat!(
        "- 2026-08-05 14:10:02 **Added**: dark theme toggle.\n",
        "- 2026-08-05 14:10:15 **Fixed**: crash when opening empty folder.\n",
        "- 2026-08-05 14:10:29 **Changed**: default sort order to newest first.\n",
        "- 2026-08-05 14:10:44 **Fixed**: broken link in settings footer.\n",
    );
    let reverse_changelog = concat!(
        "- 2026-08-08 16:40:12 **Parser**: Support nested quote blocks.\n",
        "- 2026-08-07 11:20:05 **Indexer**: Skip zero-byte lock files.\n",
        "- 2026-08-06 09:15:44 **Parser**: Handle UTF-16 BOM cleanly.\n",
        "- 2026-08-05 18:02:31 **Indexer**: Compact stale tombstones on startup.\n",
    );
    let ops_incident = concat!(
        "14:00:02 Gateway: upstream returned 502 on health check\n",
        "14:00:15 Database: replica lag rose to 14 seconds\n",
        "14:00:29 Gateway: shifted traffic to secondary pool\n",
        "14:00:41 Database: replica caught up and lag cleared\n",
    );
    let signal_changelog = concat!(
        "[2026-08-05 09:00:00] Fixed: login loop on slow links\n",
        "[2026-08-06 09:00:00] Added: export to CSV\n",
        "[2026-08-07 09:00:00] Fixed: crash on empty input\n",
        "[2026-08-08 09:00:00] Added: dark mode\n",
    );
    let meeting_agenda = concat!(
        "# Offsite plan\n",
        "Date: 2026-08-05\n",
        "Attendees: Lena, Hugo\n\n",
        "09:00:00 Lena (lead): opens the shop\n",
        "09:30:00 Hugo (ops): checks deliveries\n",
        "10:00:00 Lena (lead): calls supplier\n",
        "10:30:00 Hugo (ops): packs orders\n",
    );
    let meeting_agenda_punctuated = concat!(
        "# Offsite plan\n",
        "Date: 2026-08-05\n",
        "Attendees: Lena, Hugo\n\n",
        "09:00:00 Lena (lead): Opens the shop.\n",
        "09:30:00 Hugo (ops): Checks deliveries.\n",
        "10:00:00 Lena (lead): Calls the supplier.\n",
        "10:30:00 Hugo (ops): Packs the orders.\n",
    );
    let clock_dash_agenda = concat!(
        "09:00 - Lena: Opens with the budget.\n",
        "09:30 - Hugo: Runs the supplier review.\n",
        "10:15 - Lena: Wraps up with next steps.\n",
    );
    let time_first_roster = concat!(
        "09:00 - Lena\n",
        "opens with the budget\n\n",
        "09:30 - Hugo\n",
        "supplier review\n\n",
        "10:15 - Lena\n",
        "wrap up\n",
    );
    let pair_chores = concat!(
        "Lena: book the conference room\n",
        "Hugo: send the revised draft\n",
        "Lena: check the Q3 budget\n",
        "Hugo: confirm the catering count\n",
        "Lena: update the onboarding wiki\n",
        "Hugo: call the venue manager\n",
    );
    let expected_actual = concat!(
        "**Expected:** The export writes one UTF-8 file.\n",
        "**Actual:** The export writes two files with a BOM.\n",
        "**Expected:** Timestamps stay in UTC.\n",
        "**Actual:** Timestamps shift to local offset.\n",
        "**Expected:** Empty lines are preserved.\n",
        "**Actual:** Trailing blank lines are trimmed.\n",
    );
    let home_away_punctuated = concat!(
        "Home: Won 3-1 against Leeds.\n",
        "Away: Lost 0-2 at Hull.\n",
        "Home: Drew 1-1 with York.\n",
        "Away: Won 2-0 at Bath.\n",
        "Home: Won 4-0 against Hove.\n",
        "Away: Drew 0-0 at Ryde.\n",
    );

    for (name, text) in [
        ("agenda-plain.txt", agenda_plain),
        ("midnight-shifts.md", midnight_shifts),
        ("afternoon-agenda.md", afternoon_agenda),
        ("range-roster.txt", range_roster),
        ("same-day-changelog.md", same_day_changelog),
        ("reverse-changelog.md", reverse_changelog),
        ("ops-incident.log", ops_incident),
        ("signal-changelog.md", signal_changelog),
        ("meeting-agenda.md", meeting_agenda),
        ("meeting-agenda-punctuated.md", meeting_agenda_punctuated),
        ("clock-dash-agenda.txt", clock_dash_agenda),
        ("time-first-roster.txt", time_first_roster),
        ("pair-chores.txt", pair_chores),
        ("expected-actual.md", expected_actual),
        ("home-away-punctuated.txt", home_away_punctuated),
    ] {
        expected(name, text, Kind::Notes, &["note_default"]);
    }
}

#[test]
fn midnight_recordings_headed_clocks_and_partial_punctuation_stay_transcripts() {
    let midnight_dated_bold = concat!(
        "- 2026-08-05 23:59:45 **Speaker 1**: Ready to close out the late shift?\n",
        "- 2026-08-06 00:00:05 **Speaker 2**: Yes, all kiln temperatures dropped on schedule.\n",
        "- 2026-08-06 00:00:22 **Speaker 1**: Great, let us log the reading and lock up.\n",
        "- 2026-08-06 00:00:39 **Speaker 2**: Done, see you at eight tomorrow.\n",
    );
    let headed_clock_meeting = concat!(
        "Meeting: Greenhouse Roof Kickoff\n",
        "Date: 2026-10-05\n\n",
        "00:00:01 Speaker 1 (Sonia): Good morning everyone, thanks for joining.\n",
        "00:00:15 Speaker 2 (Devon): Morning Sonia, glad to get started.\n",
        "00:00:24 Speaker 1 (Sonia): Let us walk through the timber schedule first.\n",
    );
    let casual_partial_punctuation = concat!(
        "Lena: hey are you coming?\n",
        "Hugo: yeah leaving now\n",
        "Lena: cool bring the keys\n",
        "Hugo: got them\n",
        "Lena: need anything else?\n",
        "Hugo: nah see you in ten\n",
    );

    for (name, text, rule) in [
        (
            "midnight-dated-bold.md",
            midnight_dated_bold,
            "dated_bold_speaker",
        ),
        (
            "headed-clock-meeting.txt",
            headed_clock_meeting,
            "meeting_export_turns",
        ),
        (
            "casual-partial.txt",
            casual_partial_punctuation,
            "bare_speaker",
        ),
    ] {
        expected(name, text, Kind::Transcripts, &[rule]);
    }
}

fn add_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], value: &[u8]) {
    png.extend(u32::try_from(value.len()).unwrap().to_be_bytes());
    png.extend(kind);
    png.extend(value);
    // The minimal PNGs in these tests exercise metadata only.
    png.extend([0; 4]);
}

#[test]
fn exact_dated_screenshot_names_classify_untagged_images() {
    let png = b"\x89PNG\r\n\x1a\n";
    let jpeg = b"\xff\xd8\xff\xe0\0\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xff\xd9";
    for (name, rule) in [
        (
            "screenshot_20240129_183515.png",
            "android_spectacle_screenshot_name",
        ),
        (
            "Screenshot_2024-02-29_at_4.48.30 PM.png",
            "macos_screenshot_name",
        ),
        (
            "bildschirmfoto_2025-01-03_um_13.37.08.png",
            "macos_screenshot_name",
        ),
        (
            "Capture d’écran 2024-03-29 à 15.06.33.png",
            "macos_screenshot_name",
        ),
        (
            "снимок_экрана_2025-03-15_в_13.50.20.png",
            "macos_screenshot_name",
        ),
        ("截屏2025-05-08_21.43.39.png", "macos_screenshot_name"),
        (
            "任意の名前 2026-01-04 093845.png",
            "snipping_tool_screenshot_name",
        ),
        (
            "any_name_2026-01-04_093845_(2).png",
            "snipping_tool_screenshot_name",
        ),
        (
            "Screenshot 2026-01-04 09.38.45.png",
            "chromeos_screenshot_name",
        ),
        ("20240808_15h07m03s_grim.png", "grim_screenshot_name"),
        ("2025-06-28-022453_hyprshot.png", "hyprshot_screenshot_name"),
        (
            "2019-10-25-224521_1366x768_scrot.png",
            "scrot_screenshot_name",
        ),
        (
            "CleanShot_2024-06-18_at_11.15.21@2x.png",
            "cleanshot_screenshot_name",
        ),
        (
            "2025-01-28_09_23_18-Microsoft_PowerToys_Greenshot.png",
            "greenshot_screenshot_name",
        ),
        (
            "screencapture-example-org-docs-2025-08-31-22_24_35.png",
            "gofullpage_screenshot_name",
        ),
    ] {
        expected(name, png, Kind::Screen, &[rule]);
    }
    for (name, rule) in [
        ("Screenshot_20240208-150745.jpg", "android_screenshot_name"),
        (
            "Screenshot_20261119_081744_TalkBack.jpg",
            "android_app_screenshot_name",
        ),
        (
            "Screenshot_2026-11-04-16-08-40-356_com.android.chrome.jpg",
            "xiaomi_screenshot_name",
        ),
    ] {
        expected(name, jpeg, Kind::Screen, &[rule]);
    }
    for name in [
        "IMG_1234.PNG",
        "Screenshot (32).png",
        "Captura de pantalla (31).png",
        "Screenshot from 2026-11-19 07-18-37.png",
        "Screenshot desktop.png",
        "QQ截图20220430022000.png",
        "1597750732012 CamScanner 08-18-2020 13.27.53 1.png",
        "2026-01-04 093845.png",
        "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿.png",
        "Capture 2026-02-29 093845.png",
        "Capture 2026-01-04 246045.png",
        "Capture 2026-01-04 093860.png",
        "Screenshot 2026-01-04 at 00.38.45 PM.png",
        "Screenshot 2026-01-04 at 19.38.45 PM.png",
        "Capture d’écran 2026-01-04 um 09.38.45.png",
    ] {
        expected(name, png, Kind::Photos, &["image_default"]);
    }
    expected(
        "Capture 2026-01-04 093845.png",
        jpeg,
        Kind::Photos,
        &["image_default"],
    );
    expected(
        "Photo from 2026-11-19 08-00-00.1.jpeg",
        jpeg,
        Kind::Photos,
        &["image_default"],
    );
}

#[test]
fn screenshot_png_software_and_coordinates_are_exact() {
    let png = |chunks: &[(&[u8; 4], &[u8])]| {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        for &(kind, data) in chunks {
            add_png_chunk(&mut bytes, kind, data);
        }
        bytes
    };
    for image in [
        png(&[(b"tEXt", b"Software\0Greenshot")]),
        png(&[(b"iTXt", b"Software\0\0\0\0\0Greenshot")]),
    ] {
        expected(
            "neutral.png",
            image,
            Kind::Screen,
            &["greenshot_screenshot_software"],
        );
    }
    let x = (b"tEXt", b"logicalX\x0012".as_slice());
    let y = (b"tEXt", b"logicalY\x00-4".as_slice());
    expected(
        "neutral.png",
        png(&[x, y]),
        Kind::Screen,
        &["spectacle_screenshot_coordinates"],
    );
    for image in [
        png(&[x]),
        png(&[y]),
        png(&[(b"tEXt", b"Software\0Greenshot copy")]),
        png(&[(b"tEXt", b"Comment\0Greenshot")]),
    ] {
        expected("neutral.png", image, Kind::Photos, &["image_default"]);
    }
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

    let inline = r#"<r:Description xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://ns.adobe.com/exif/1.0/" ex:UserComment="Screenshot"/>"#;
    expected(
        "neutral.png",
        xmp_png(inline),
        Kind::Screen,
        &["screenshot_user_comment"],
    );
    for other in [
        inline.replace("ex:UserComment", "UserComment"),
        inline.replace("Screenshot", "Screenshot copy"),
        inline.replace("http://ns.adobe.com/exif/1.0/", "urn:unrelated"),
    ] {
        expected(
            "neutral.png",
            xmp_png(&other),
            Kind::Photos,
            &["image_default"],
        );
    }

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

    // ImageDescription in IFD0, independent of the UserComment field.
    let description =
        b"II*\0\x08\0\0\0\x01\0\x0e\x01\x02\0\x0b\0\0\0\x1a\0\0\0\0\0\0\0Screenshot\0";
    expected(
        "neutral.png",
        png(b"eXIf", description),
        Kind::Screen,
        &["screenshot_user_comment"],
    );
    let mut changed = *description;
    *changed.last_mut().unwrap() = b'!';
    expected(
        "neutral.png",
        png(b"eXIf", &changed),
        Kind::Photos,
        &["image_default"],
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

#[test]
fn structured_chat_and_meeting_exports_classify_as_transcripts() {
    let telegram_json = r#"{
        "name": "Boat Repair",
        "type": "personal_chat",
        "id": 9001,
        "messages": [
            {"id": 1, "type": "service", "date": "2026-04-01T08:00:00", "date_unixtime": "1775030400", "action": "create_group", "text": ""},
            {"id": 2, "type": "message", "date": "2026-04-01T08:05:00", "date_unixtime": "1775030700", "from": "Mira", "from_id": "user101", "text": "Did the varnish dry?"},
            {"id": 3, "type": "message", "date": "2026-04-01T08:06:10", "date_unixtime": "1775030770", "from": null, "from_id": "user202", "text": ["Almost, check ", {"type": "bold", "text": "port side"}, "."]}
        ]
    }"#;
    let telegram_html = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Exported Data</title></head>
<body><div class="page_wrap"><div class="page_body chat_page"><div class="history">
<div class="message service" id="message-1"><div class="body details">1 April 2026</div></div>
<div class="message default clearfix" id="message1"><div class="body">
<div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div>
<div class="from_name">Mira</div>
<div class="text">Did the varnish &amp; primer dry?<br>Check the bow.</div>
</div></div>
<div class="message default clearfix joined" id="message2"><div class="body">
<div class="pull_right date details" title="01.04.2026 11:06:10 UTC+03:00">11:06</div>
<div class="text">Looks ready.</div>
</div></div>
</div></div></div></body></html>"#;
    let slack_named = r#"[
        {"type":"message","user":"U111AAA","text":"Bringing the planks.","ts":"1743494400.000100","user_profile":{"real_name":"Mira Alder"}},
        {"type":"message","user":"U222BBB","text":"Great, dock four.","ts":"1743494460.000200","user_profile":{"display_name":"Theo"}}
    ]"#;
    let slack_ids_only = r#"[
        {"type":"message","user":"U111AAA","text":"Gate is open.","ts":"1743494400.000100"},
        {"type":"message","user":"U222BBB","text":"On my way.","ts":"1743494460.000200"}
    ]"#;
    let fireflies_csv = "Speaker,Start Time,End Time,Sentence\r\nMira,00:00:01,00:00:03,\"Hello, \"\"team\"\"\"\r\nTheo,00:00:04,00:00:07,\"Ready for the list.\"\r\n";
    let macwhisper_json = r#"{
        "text": "Hello everyone. Let us begin.",
        "segments": [
            {"id": "11111111-2222-3333-4444-555555555555", "start": 0, "end": 1800, "text": "Hello everyone.", "speaker": "Speaker 1"},
            {"id": "66666666-7777-8888-9999-aaaaaaaaaaaa", "start": 1900, "end": 3500, "text": "Let us begin.", "speaker": "Speaker 2"}
        ]
    }"#;
    let podcast_json = r#"{
        "version": "1.0.0",
        "language": "en",
        "segments": [
            {"speaker": "Mira", "startTime": 0.0, "endTime": 1.2, "body": "Welcome "},
            {"speaker": "Mira", "startTime": 1.2, "endTime": 2.4, "body": "aboard."},
            {"speaker": "Theo", "startTime": 2.5, "endTime": 4.0, "body": "Glad to be here."}
        ]
    }"#;

    for (name, body, rule) in [
        ("result.json", telegram_json, "telegram_json"),
        ("messages.html", telegram_html, "telegram_html"),
        ("2026-04-01.json", slack_named, "slack_json"),
        ("dock.json", slack_ids_only, "slack_json"),
        ("retro.csv", fireflies_csv, "fireflies_csv"),
        ("meeting.json", macwhisper_json, "macwhisper_json"),
        ("episode.json", podcast_json, "podcast_transcript_json"),
    ] {
        expected(name, body, Kind::Transcripts, &[rule]);
    }
}

#[test]
fn non_export_and_out_of_scope_structured_files_do_not_classify_as_transcripts() {
    let config_json = r#"{"host":"127.0.0.1","port":8080,"features":{"search":true}}"#;
    let table_csv = "item,qty,unit_price\nbolts,12,3.50\nnuts,24,1.25\n";
    let table_tsv = "item\tqty\tunit_price\nbolts\t12\t3.50\nnuts\t24\t1.25\n";
    let web_page_html = "<!DOCTYPE html><html><head><title>Weather</title></head><body><main><h1>Harbor Forecast</h1><p>Light winds from the north.</p></main></body></html>";
    let broken_json = "{\"items\":[{\"text\":\"line one \\\n line two\"}]}";
    let fathom_api_json = r#"{"items":[{"title":"Weekly Sync","default_summary":{"markdown_formatted":"Summary"},"calendar_invitees":[{"name":"Mira"}],"transcript":[{"speaker":{"display_name":"Mira"},"text":"Hi","timestamp":"00:00:01"}]}]}"#;
    let fireflies_graphql_json = r#"{"data":{"transcript":{"title":"Review","summary":{"overview":"Done"},"sentences":[{"speaker_name":"Mira","text":"Hello","start_time":1.0,"end_time":2.0}]}}}"#;
    let tldv_api_json =
        r#"{"id":"m1","data":[{"speaker":"Mira","text":"Hello","startTime":1,"endTime":3}]}"#;
    let scraper_json = r#"{"metadata":{"title":"Sync"},"transcript":[{"speaker":"Mira","timestamp":"00:01","text":"Hello"}]}"#;
    let agent_jsonl = "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":\"hello\"}}\n";
    let telegram_whole_account = r#"{"personal_information":{"user_id":1,"phone_number":"+1000"},"chats":{"about":"List","list":[{"name":"A","type":"personal_chat","id":1,"messages":[{"id":1,"type":"message","date":"2026-04-01T08:00:00","date_unixtime":"1775030400","from":"Mira","text":"Hi"}]}]}}"#;
    let telegram_batch_list = r#"{"chats":{"list":[{"name":"A","type":"personal_chat","id":1,"messages":[{"id":1,"type":"message","date":"2026-04-01T08:00:00","date_unixtime":"1775030400","from":"Mira","text":"Hi"}]}]}}"#;
    let chatgpt_bare = r#"[{"title":"Notes","create_time":1700000000,"mapping":{"a":{"id":"a","message":{"author":{"role":"user"},"content":{"parts":["hello"]}}}}}]"#;
    let claude_bare = r#"[{"uuid":"1111","name":"Chat","chat_messages":[{"uuid":"2222","sender":"human","text":"hello"}]}]"#;
    let gemini_bare = r#"[{"header":"Gemini Apps","title":"Prompted hello","time":"2026-04-01T12:00:00Z","safeHtmlItem":[{"html":"<p>answer</p>"}]}]"#;
    let granola_csv = "id,title,created_at,summary,notes,transcript\n1,Sync,2026-04-01,Summary,Notes,\"Mira: Hi\"\n";

    for (name, body) in [
        ("config.json", config_json),
        ("table.csv", table_csv),
        ("table.tsv", table_tsv),
        ("page.html", web_page_html),
        ("broken.json", broken_json),
        ("fathom.json", fathom_api_json),
        ("fireflies-api.json", fireflies_graphql_json),
        ("tldv-api.json", tldv_api_json),
        ("scraper.json", scraper_json),
        ("session.jsonl", agent_jsonl),
        ("account.json", telegram_whole_account),
        ("batch.json", telegram_batch_list),
        ("conversations.json", chatgpt_bare),
        ("claude.json", claude_bare),
        ("gemini.json", gemini_bare),
        ("granola.csv", granola_csv),
    ] {
        let found = verdict(name, body);
        assert_ne!(
            found.kind,
            Kind::Transcripts,
            "{name} must not classify as transcripts (got {:?})",
            found.reasons
        );
    }
}

#[test]
fn pathological_structured_inputs_are_refused_within_bounds() {
    let deep_json = format!("{}{}{}", "[".repeat(80), "1", "]".repeat(80));
    let huge_csv_cell = format!(
        "Speaker,Start Time,End Time,Sentence\nMira,00:00:01,00:00:02,\"{}\"\n",
        "x".repeat(9 * 1024)
    );
    let unclosed_csv =
        "Speaker,Start Time,End Time,Sentence\nMira,00:00:01,00:00:02,\"unterminated\n";
    let wide_csv = format!(
        "Speaker,Start Time,End Time,Sentence{}\nMira,00:00:01,00:00:02,Hi{}\n",
        ",extra".repeat(20),
        ",v".repeat(20)
    );
    let huge_tsv_cell = format!("start\tend\ttext\n0\t1000\t{}\n", "x".repeat(9 * 1024));
    let deep_html = format!(
        "<!DOCTYPE html><html><body><div class=\"page_body chat_page\"><div class=\"history\">{}{}{}</div></div></body></html>",
        "<div>".repeat(80),
        r#"<div class="message default"><div class="body"><div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div><div class="from_name">Mira</div><div class="text">Hi</div></div></div>"#,
        "</div>".repeat(80)
    );
    let huge_html_tag = format!(
        r#"<!DOCTYPE html><html><body><div class="page_body chat_page"><div class="history"><div class="message default" data-pad="{}"><div class="body"><div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div><div class="from_name">Mira</div><div class="text">Hi</div></div></div></div></div></body></html>"#,
        "a".repeat(20 * 1024)
    );
    let truncated_html = r#"<!DOCTYPE html><html><body><div class="page_body chat_page"><div class="history"><div class="message default"><div class="body"><div class="text">unterminated"#;
    let many_attrs_html = format!(
        r#"<!DOCTYPE html><html><body><div class="page_body chat_page"><div class="history"><div class="message default" {}><div class="body"><div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div><div class="from_name">Mira</div><div class="text">Hi</div></div></div></div></div></body></html>"#,
        (0..40)
            .map(|i| format!(r#"a{i}="1""#))
            .collect::<Vec<_>>()
            .join(" ")
    );

    for (name, body) in [
        ("deep.json", deep_json.as_str()),
        ("huge.csv", huge_csv_cell.as_str()),
        ("unclosed.csv", unclosed_csv),
        ("wide.csv", wide_csv.as_str()),
        ("huge.tsv", huge_tsv_cell.as_str()),
        ("deep.html", deep_html.as_str()),
        ("huge-tag.html", huge_html_tag.as_str()),
        ("truncated.html", truncated_html),
        ("many-attrs.html", many_attrs_html.as_str()),
    ] {
        let found = verdict(name, body);
        assert_ne!(
            found.kind,
            Kind::Transcripts,
            "{name} must be refused within safety bounds (got {:?})",
            found.reasons
        );
    }
}

#[test]
fn structured_document_extensions_preserve_signatures_beat_dialogue_turns_and_keep_binary_precedence()
 {
    // 1. Real .eml with interleaved Received/DKIM headers and dialogue lines in body
    //    must classify as Documents (not hijacked by Stage::Turns).
    let interleaved_eml = concat!(
        "Received: from mail.example.org by mx.example.com; Fri, 01 May 2026 09:00:00 +0000\r\n",
        "DKIM-Signature: v=1; a=rsa-sha256; d=example.org; s=sel1; bh=abc; b=def\r\n",
        "Message-ID: <abc@example.org>\r\n",
        "From: Alice <alice@example.org>\r\n",
        "MIME-Version: 1.0\r\n",
        "To: Bob <bob@example.org>\r\n",
        "Subject: Notes\r\n\r\n",
        "Alice: Did we finalize the draft?\r\n",
        "Bob: Yes, ready for Monday.\r\n",
        "Alice: Great, thanks.\r\n",
        "Bob: See you then.\r\n"
    );
    let eml_v = verdict("thread.eml", interleaved_eml);
    assert_eq!(eml_v.kind, Kind::Documents, "{:?}", eml_v.reasons);
    assert_eq!(eml_v.ruleset_version, RULESET_VERSION);

    // 2. General .html/.htm/.csv/.tsv containing dialogue-like colon lines classify as Documents.
    let dialogue_html = "<html><body><p>Alice: Hello there.</p><p>Bob: Hi Alice.</p><p>Alice: Ready?</p><p>Bob: Yes.</p></body></html>";
    let dialogue_csv = "Speaker,Line\nAlice: Hello,1\nBob: Hi,2\nAlice: Ready,3\nBob: Yes,4\n";
    let dialogue_tsv = "Speaker\tLine\nAlice: Hello\t1\nBob: Hi\t2\nAlice: Ready\t3\nBob: Yes\t4\n";
    for (name, body) in [
        ("page.html", dialogue_html),
        ("page.htm", dialogue_html),
        ("notes.csv", dialogue_csv),
        ("notes.tsv", dialogue_tsv),
    ] {
        let v = verdict(name, body);
        assert_eq!(v.kind, Kind::Documents, "{name}: {:?}", v.reasons);
        assert_eq!(v.reasons, vec!["structured_document_extension"], "{name}");
    }

    // 3. Recognized transcript signatures STILL win in Stage::Signature with single-element reasons.
    let fireflies = "Speaker,Start Time,End Time,Sentence\nMira,00:00:01,00:00:04,Ready to start.\nTheo,00:00:05,00:00:08,Yes.\n";
    expected("call.csv", fireflies, Kind::Transcripts, &["fireflies_csv"]);
    let whisper = "start\tend\ttext\n0\t1200\tWelcome everyone.\n1300\t2600\tLet us begin.\n";
    expected("call.tsv", whisper, Kind::Transcripts, &["whisper_tsv"]);
    let telegram = r#"<!DOCTYPE html><html><body><div class="page_body chat_page"><div class="history"><div class="message default"><div class="body"><div class="pull_right date details" title="01.04.2026 11:05:00 UTC+03:00">11:05</div><div class="from_name">Mira</div><div class="text">Ready?</div></div></div></div></div></body></html>"#;
    expected(
        "messages.html",
        telegram,
        Kind::Transcripts,
        &["telegram_html"],
    );

    // 4. Binary image bytes renamed .html/.csv/.eml still classify as images (bytes beat names).
    let png = b"\x89PNG\r\n\x1a\n";
    for name in ["photo.html", "photo.csv", "photo.eml"] {
        let v = verdict(name, png);
        assert!(
            matches!(v.kind, Kind::Photos | Kind::Screen),
            "{name} should classify as image, got {:?} ({:?})",
            v.kind,
            v.reasons
        );
    }
}
