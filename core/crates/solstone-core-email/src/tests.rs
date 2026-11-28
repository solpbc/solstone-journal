// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unit and integration tests for `solstone-core-email`, including an
//! in-memory RFC 3501 + Gmail mock IMAP server and a synthetic labelled
//! extraction corpus proving zero owner-written spans lost.

use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use tempfile::TempDir;

use crate::choice::{compute_accounting, detect_folder_cap};
use crate::error::EmailError;
use crate::extract::{self, CAP_CHARS, Evidence, cap_text, owner_fallback_text};
use crate::imap::{GmailImap, ImapSource};
use crate::model::{MessageType, RangePreset};
use crate::select::{LeftOutBucket, ParentMatchRule};
use crate::store;
use crate::sync::{self, SyncOptions};
use crate::text::loose;

// ---------------------------------------------------------------------------
// In-memory duplex stream & mock RFC 3501 + Gmail IMAP server
// ---------------------------------------------------------------------------

struct ChannelStream {
    rx: mpsc::Receiver<Vec<u8>>,
    tx: mpsc::Sender<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let avail = &self.buf[self.pos..];
        let n = avail.len().min(out.len());
        out[..n].copy_from_slice(&avail[..n]);
        self.pos += n;
        Ok(n)
    }
}

impl Write for ChannelStream {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.tx
            .send(data.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"))?;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn duplex_streams() -> (ChannelStream, ChannelStream) {
    let (a_tx, b_rx) = mpsc::channel();
    let (b_tx, a_rx) = mpsc::channel();
    (
        ChannelStream {
            rx: a_rx,
            tx: a_tx,
            buf: Vec::new(),
            pos: 0,
        },
        ChannelStream {
            rx: b_rx,
            tx: b_tx,
            buf: Vec::new(),
            pos: 0,
        },
    )
}

#[derive(Clone)]
struct MockMessage {
    uid: u32,
    gm_msgid: String,
    gm_thrid: String,
    labels: Vec<String>,
    flags: Vec<String>,
    internal_date: String,
    bodystructure: String,
    raw_headers: String,
    /// `(part_path, mime_header, body_bytes)`
    parts: Vec<(String, String, String)>,
}

#[derive(Clone)]
struct MockServerConfig {
    expected_user: String,
    expected_pass: String,
    imap_disabled: bool,
    hide_all_mail: bool,
    uid_validity: u32,
    exists_override: Option<u32>,
    messages: Vec<MockMessage>,
}

impl MockServerConfig {
    fn spawn(self) -> (ChannelStream, thread::JoinHandle<Vec<String>>) {
        let (client_stream, server_stream) = duplex_streams();
        let handle = thread::spawn(move || serve_mock_imap(server_stream, self));
        (client_stream, handle)
    }
}

fn serve_mock_imap(stream: ChannelStream, cfg: MockServerConfig) -> Vec<String> {
    let mut reader = BufReader::new(stream);
    let mut commands = Vec::new();
    let send = |r: &mut BufReader<ChannelStream>, s: &[u8]| {
        let _ = r.get_mut().write_all(s);
        let _ = r.get_mut().flush();
    };
    send(&mut reader, b"* OK Gimap ready for requests\r\n");

    let mut line = String::new();
    loop {
        line.clear();
        let Ok(n) = reader.read_line(&mut line) else {
            break;
        };
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        commands.push(trimmed.clone());
        let Some((tag, rest)) = trimmed.split_once(' ') else {
            break;
        };
        let upper = rest.to_ascii_uppercase();
        if upper.starts_with("LOGIN ") {
            if cfg.imap_disabled {
                send(
                    &mut reader,
                    format!("{tag} NO [ALERT] Your account is not enabled for IMAP use.\r\n")
                        .as_bytes(),
                );
            } else if rest.contains(&cfg.expected_user) && rest.contains(&cfg.expected_pass) {
                send(
                    &mut reader,
                    format!("{tag} OK authenticated (Success)\r\n").as_bytes(),
                );
            } else {
                send(
                    &mut reader,
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
                        .as_bytes(),
                );
            }
        } else if upper.starts_with("LIST ") {
            send(&mut reader, b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n");
            if !cfg.hide_all_mail {
                send(
                    &mut reader,
                    b"* LIST (\\All \\HasNoChildren) \"/\" \"[Gmail]/All Mail\"\r\n",
                );
            }
            send(&mut reader, format!("{tag} OK Success\r\n").as_bytes());
        } else if upper.starts_with("EXAMINE ") {
            let exists = cfg.exists_override.unwrap_or(cfg.messages.len() as u32);
            let uid_next = cfg
                .messages
                .iter()
                .map(|m| m.uid)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            send(&mut reader, format!("* {exists} EXISTS\r\n").as_bytes());
            send(
                &mut reader,
                format!("* OK [UIDVALIDITY {}] UIDs valid.\r\n", cfg.uid_validity).as_bytes(),
            );
            send(
                &mut reader,
                format!("* OK [UIDNEXT {uid_next}] Predicted next UID.\r\n").as_bytes(),
            );
            send(
                &mut reader,
                format!("{tag} OK [READ-ONLY] [Gmail]/All Mail selected. (Success)\r\n").as_bytes(),
            );
        } else if upper.starts_with("UID FETCH ") {
            let after = &rest["UID FETCH ".len()..];
            let (uid_spec, attrs_spec) = after.split_once(' ').unwrap_or((after, ""));
            let matched_uids = parse_mock_uid_spec(uid_spec);
            let attrs_upper = attrs_spec.to_ascii_uppercase();
            let wants_headers = attrs_upper.contains("BODY.PEEK[HEADER.FIELDS");
            for (idx, msg) in cfg.messages.iter().enumerate() {
                if !uid_matches(msg.uid, &matched_uids) {
                    continue;
                }
                let seq = idx + 1;
                if wants_headers {
                    let labels = msg
                        .labels
                        .iter()
                        .map(|l| {
                            if l.contains(' ') {
                                format!("\"{l}\"")
                            } else {
                                l.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let flags = msg.flags.join(" ");
                    let hdr_bytes = msg.raw_headers.as_bytes();
                    let prefix = format!(
                        "* {seq} FETCH (UID {} FLAGS ({flags}) INTERNALDATE \"{}\" RFC822.SIZE 1024 X-GM-MSGID {} X-GM-THRID {} X-GM-LABELS ({labels}) BODYSTRUCTURE {} BODY[HEADER.FIELDS (MESSAGE-ID IN-REPLY-TO REFERENCES FROM TO CC DATE SUBJECT AUTO-SUBMITTED LIST-ID)] {{{}}}\r\n",
                        msg.uid,
                        msg.internal_date,
                        msg.gm_msgid,
                        msg.gm_thrid,
                        msg.bodystructure,
                        hdr_bytes.len()
                    );
                    send(&mut reader, prefix.as_bytes());
                    send(&mut reader, hdr_bytes);
                    send(&mut reader, b")\r\n");
                } else {
                    // Body part fetch: look up requested part number.
                    let mut resp_buf = Vec::new();
                    resp_buf
                        .extend_from_slice(format!("* {seq} FETCH (UID {}", msg.uid).as_bytes());
                    for (part_num, mime_hdr, body_str) in &msg.parts {
                        let peek_mime = format!("BODY.PEEK[{part_num}.MIME]");
                        let peek_body = format!("BODY.PEEK[{part_num}]");
                        if attrs_upper.contains(&peek_mime) {
                            resp_buf.extend_from_slice(
                                format!(" BODY[{part_num}.MIME] {{{}}}\r\n", mime_hdr.len())
                                    .as_bytes(),
                            );
                            resp_buf.extend_from_slice(mime_hdr.as_bytes());
                        }
                        if attrs_upper.contains(&peek_body) {
                            resp_buf.extend_from_slice(
                                format!(" BODY[{part_num}] {{{}}}\r\n", body_str.len()).as_bytes(),
                            );
                            resp_buf.extend_from_slice(body_str.as_bytes());
                        }
                    }
                    resp_buf.extend_from_slice(b")\r\n");
                    send(&mut reader, &resp_buf);
                }
            }
            send(&mut reader, format!("{tag} OK Success\r\n").as_bytes());
        } else if upper.starts_with("LOGOUT") {
            send(&mut reader, b"* BYE LOGOUT Requested\r\n");
            send(&mut reader, format!("{tag} OK 73 good day\r\n").as_bytes());
            break;
        } else {
            send(&mut reader, format!("{tag} BAD unsupported\r\n").as_bytes());
        }
    }
    commands
}

enum UidFilter {
    Single(u32),
    Range(u32, u32),
}

fn parse_mock_uid_spec(spec: &str) -> Vec<UidFilter> {
    spec.split(',')
        .filter_map(|part| {
            if let Some((a, b)) = part.split_once(':') {
                Some(UidFilter::Range(a.parse().ok()?, b.parse().ok()?))
            } else {
                Some(UidFilter::Single(part.parse().ok()?))
            }
        })
        .collect()
}

fn uid_matches(uid: u32, filters: &[UidFilter]) -> bool {
    filters.iter().any(|f| match *f {
        UidFilter::Single(v) => uid == v,
        UidFilter::Range(lo, hi) => uid >= lo && uid <= hi,
    })
}

fn plain_bodystructure(size: usize) -> String {
    format!("(\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" {size} 4 NIL NIL NIL)")
}

fn sample_mailbox_messages() -> Vec<MockMessage> {
    let body1 = "Could we meet on Thursday afternoon to review the agenda?\r\n";
    let body2 = "Yes, the afternoon works.\r\n\r\nI've attached the revised agenda.\r\n\r\nOn Tue, Nov 17, 2026 at 4:20 PM Casey <casey@example.com> wrote:\r\n> Could we meet on Thursday afternoon to review the agenda?\r\n";
    let body3 = "Sending over the venue notes for Saturday.\r\n";
    let body4 = "Looping you in on the room booking.\r\n\r\n---------- Forwarded message ---------\r\nFrom: Jordan <jordan@example.com>\r\nDate: Mon, Nov 16, 2026 at 10:00 AM\r\nSubject: Room schedule\r\nTo: Maya Chen <maya@example.com>\r\n\r\nRoom seven is free at two.\r\n";
    let body5 = "Confirmed via References only — see you at noon.\r\n";
    let body_old = "Old message from two years ago.\r\n";
    vec![
        // UID 1: Incoming message from Casey (answered by UID 2 and UID 5).
        MockMessage {
            uid: 1,
            gm_msgid: "1001".into(),
            gm_thrid: "t1".into(),
            labels: vec!["\\Inbox".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "17-Nov-2026 16:20:00 -0700".into(),
            bodystructure: plain_bodystructure(body1.len()),
            raw_headers: "Message-ID: <parent-1@example.com>\r\nFrom: Casey <casey@example.com>\r\nTo: Maya Chen <maya@example.com>\r\nDate: Tue, 17 Nov 2026 16:20:00 -0700\r\nSubject: Thursday meeting\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body1.into())],
        },
        // UID 2: Owner reply with In-Reply-To + attachment + inline image.
        MockMessage {
            uid: 2,
            gm_msgid: "1002".into(),
            gm_thrid: "t1".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "18-Nov-2026 10:16:00 -0700".into(),
            bodystructure: format!(
                "((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" {} 5 NIL NIL NIL)(\"APPLICATION\" \"PDF\" (\"NAME\" \"agenda.pdf\") NIL NIL \"BASE64\" 4096 NIL (\"ATTACHMENT\" (\"FILENAME\" \"agenda.pdf\")) NIL)(\"IMAGE\" \"JPEG\" (\"NAME\" \"site-photo.jpg\") \"<img1>\" NIL \"BASE64\" 2048 NIL (\"INLINE\" (\"FILENAME\" \"site-photo.jpg\")) NIL) \"MIXED\" (\"BOUNDARY\" \"b\") NIL NIL)",
                body2.len()
            ),
            raw_headers: "Message-ID: <reply-2@example.com>\r\nIn-Reply-To: <parent-1@example.com>\r\nReferences: <parent-1@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nCc: Lee <lee@example.com>\r\nDate: Wed, 18 Nov 2026 10:16:00 -0700\r\nSubject: Re: Thursday meeting\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body2.into())],
        },
        // UID 3: Owner new message.
        MockMessage {
            uid: 3,
            gm_msgid: "1003".into(),
            gm_thrid: "t3".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "19-Nov-2026 09:00:00 -0700".into(),
            bodystructure: plain_bodystructure(body3.len()),
            raw_headers: "Message-ID: <new-3@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Sam <sam@example.com>\r\nDate: Thu, 19 Nov 2026 09:00:00 -0700\r\nSubject: Venue notes\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body3.into())],
        },
        // UID 4: Owner forward with comment + forwarded body.
        MockMessage {
            uid: 4,
            gm_msgid: "1004".into(),
            gm_thrid: "t4".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "20-Nov-2026 11:30:00 -0700".into(),
            bodystructure: plain_bodystructure(body4.len()),
            raw_headers: "Message-ID: <fwd-4@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Lee <lee@example.com>\r\nDate: Fri, 20 Nov 2026 11:30:00 -0700\r\nSubject: Fwd: Room schedule\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body4.into())],
        },
        // UID 5: Owner reply matched via References only (no In-Reply-To).
        MockMessage {
            uid: 5,
            gm_msgid: "1005".into(),
            gm_thrid: "t1".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "21-Nov-2026 08:15:00 -0700".into(),
            bodystructure: plain_bodystructure(body5.len()),
            raw_headers: "Message-ID: <refonly-5@example.com>\r\nReferences: <older@example.com> <parent-1@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nDate: Sat, 21 Nov 2026 08:15:00 -0700\r\nSubject: Re: Thursday meeting\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body5.into())],
        },
        // UID 6: Left out — calendar response.
        MockMessage {
            uid: 6,
            gm_msgid: "1006".into(),
            gm_thrid: "t6".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "21-Nov-2026 09:00:00 -0700".into(),
            bodystructure: "(\"TEXT\" \"CALENDAR\" (\"CHARSET\" \"UTF-8\" \"METHOD\" \"REPLY\") NIL NIL \"7BIT\" 120 4 NIL NIL NIL)".into(),
            raw_headers: "Message-ID: <cal-6@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nDate: Sat, 21 Nov 2026 09:00:00 -0700\r\nSubject: Accepted: Standup\r\n\r\n".into(),
            parts: vec![],
        },
        // UID 7: Left out — auto-submitted reply.
        MockMessage {
            uid: 7,
            gm_msgid: "1007".into(),
            gm_thrid: "t7".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "21-Nov-2026 10:00:00 -0700".into(),
            bodystructure: plain_bodystructure(20),
            raw_headers: "Message-ID: <auto-7@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nDate: Sat, 21 Nov 2026 10:00:00 -0700\r\nSubject: Out of office\r\nAuto-Submitted: auto-replied\r\n\r\n".into(),
            parts: vec![],
        },
        // UID 8: Left out — draft.
        MockMessage {
            uid: 8,
            gm_msgid: "1008".into(),
            gm_thrid: "t8".into(),
            labels: vec!["\\Draft".into()],
            flags: vec!["\\Draft".into()],
            internal_date: "21-Nov-2026 11:00:00 -0700".into(),
            bodystructure: plain_bodystructure(20),
            raw_headers: "Message-ID: <draft-8@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nDate: Sat, 21 Nov 2026 11:00:00 -0700\r\nSubject: Unfinished note\r\n\r\n".into(),
            parts: vec![],
        },
        // UID 9: Left out — delivery report.
        MockMessage {
            uid: 9,
            gm_msgid: "1009".into(),
            gm_thrid: "t9".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "21-Nov-2026 12:00:00 -0700".into(),
            bodystructure: "((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" 10 1 NIL NIL NIL) \"REPORT\" (\"BOUNDARY\" \"r\") NIL NIL)".into(),
            raw_headers: "Message-ID: <dsn-9@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Casey <casey@example.com>\r\nDate: Sat, 21 Nov 2026 12:00:00 -0700\r\nSubject: Delivery status\r\n\r\n".into(),
            parts: vec![],
        },
        // UID 10: Older sent message (2 years old — outside 30d, inside all).
        MockMessage {
            uid: 10,
            gm_msgid: "1010".into(),
            gm_thrid: "t10".into(),
            labels: vec!["\\Sent".into()],
            flags: vec!["\\Seen".into()],
            internal_date: "15-Mar-2024 14:00:00 -0600".into(),
            bodystructure: plain_bodystructure(body_old.len()),
            raw_headers: "Message-ID: <old-10@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Alex <alex@example.com>\r\nDate: Fri, 15 Mar 2024 14:00:00 -0600\r\nSubject: Spring plans\r\n\r\n".into(),
            parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body_old.into())],
        },
    ]
}

fn mock_server(messages: Vec<MockMessage>) -> MockServerConfig {
    MockServerConfig {
        expected_user: "maya@example.com".into(),
        expected_pass: "abcd1234efgh5678".into(),
        imap_disabled: false,
        hide_all_mail: false,
        uid_validity: 777,
        exists_override: None,
        messages,
    }
}

fn now_ts() -> Timestamp {
    "2026-11-25T12:00:00-07:00".parse().unwrap()
}

fn denver_tz() -> TimeZone {
    TimeZone::get("America/Denver").unwrap()
}

#[test]
fn typed_imap_errors_classify_auth_all_mail_and_disabled() {
    // Wrong password -> AuthFailed
    let (stream, handle) = mock_server(vec![]).spawn();
    let err = GmailImap::from_stream(stream, "maya@example.com", "wrongpassword9999").err();
    assert!(matches!(err, Some(EmailError::AuthFailed)), "{err:?}");
    let _ = handle.join();

    // All Mail hidden -> AllMailHidden
    let mut cfg = mock_server(vec![]);
    cfg.hide_all_mail = true;
    let (stream, handle) = cfg.spawn();
    let err = GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").err();
    assert!(matches!(err, Some(EmailError::AllMailHidden)), "{err:?}");
    let _ = handle.join();

    // IMAP disabled -> ImapDisabled
    let mut cfg = mock_server(vec![]);
    cfg.imap_disabled = true;
    let (stream, handle) = cfg.spawn();
    let err = GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").err();
    assert!(matches!(err, Some(EmailError::ImapDisabled)), "{err:?}");
    let _ = handle.join();
}

#[test]
fn end_to_end_read_only_scan_choice_sync_widen_tombstone_delete_and_rebuild() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let msgs = sample_mailbox_messages();
    let tz = denver_tz();
    let now = now_ts();

    // 1. Connect & header scan: 0 body reads, strictly read-only wire log.
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd 1234 efgh 5678",
        &mut client,
        now,
    )
    .unwrap();
    let scan = sync::run_header_scan(root, &mut client, now, &tz, false).unwrap();
    assert!(scan.complete);
    assert_eq!(scan.checked, 10);
    assert_eq!(scan.eligible_sent, 5);
    assert_eq!(scan.left_out_total, 4);
    assert_eq!(scan.earliest_month.as_deref(), Some("march 2024"));
    assert_eq!(
        scan.wire.body_section_requests, 0,
        "scan must read 0 bodies"
    );
    assert!(scan.wire.read_only_acks >= 1);
    client.logout().unwrap();
    let wire_cmds = handle.join().unwrap();
    assert!(
        wire_cmds
            .iter()
            .all(|c| !c.contains("SELECT") && !c.contains("STORE") && !c.contains("EXPUNGE")),
        "mutating command sent: {wire_cmds:?}"
    );

    // Verify status awaiting_choice & zero landed exchanges before owner chooses (AC 2).
    let status = sync::status_snapshot(root, &tz).unwrap();
    assert_eq!(status.state, "setting_up");
    assert_eq!(status.phase, "awaiting_choice");
    assert_eq!(status.accounting.sent_total, 9);
    assert_eq!(status.accounting.in_journal, 0);
    assert_eq!(status.accounting.outside_choice, 5);
    assert_eq!(status.accounting.left_out.total, 4);
    assert_eq!(status.accounting.left_out.calendar_responses, 1);
    assert_eq!(status.accounting.left_out.automatic_replies, 1);
    assert_eq!(status.accounting.left_out.drafts, 1);
    assert_eq!(status.accounting.left_out.delivery_reports, 1);

    // Verify choice preview for 30d vs all (0 bodies read, 3-unit estimate).
    let preview_30d =
        sync::choice_preview(root, RangePreset::Last30d, &MessageType::ALL, now, &tz).unwrap();
    assert_eq!(preview_30d.ranges.last_30d, 4);
    assert_eq!(preview_30d.ranges.all, 5);
    assert_eq!(preview_30d.types_in_selected_range.replies, 2);
    assert_eq!(preview_30d.types_in_selected_range.new_messages, 1);
    assert_eq!(preview_30d.types_in_selected_range.forwards, 1);
    assert_eq!(preview_30d.estimate.segments, 4);
    assert_eq!(preview_30d.estimate.days, 4);
    assert!(preview_30d.estimate.thoughts >= 4);

    // 2. Owner chooses last 30 days, all 3 types; simulate interrupt after 2 messages then resume (AC 15).
    sync::save_choice(
        root,
        RangePreset::Last30d,
        MessageType::ALL.to_vec(),
        now,
        &tz,
    )
    .unwrap();
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let part1 = sync::run_sync(
        root,
        &mut client,
        now,
        &tz,
        SyncOptions {
            max_messages: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(part1.landed_messages, 2);
    assert!(
        !store::bodies_dir(root).exists(),
        "transient bodies must be deleted"
    );
    client.logout().unwrap();
    let _ = handle.join();

    // Resume sync -> lands remaining 2 without duplicating the first 2.
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let part2 = sync::run_sync(root, &mut client, now, &tz, SyncOptions::default()).unwrap();
    assert_eq!(part2.landed_messages, 2);
    assert_eq!(part2.skipped_already_landed, 2);
    assert!(!store::bodies_dir(root).exists());
    client.logout().unwrap();
    let _ = handle.join();

    // Check landed exchange content for reply_to (both in_reply_to & references_only),
    // forwarded text, and named attachments (never reading attachment bytes).
    let landed = store::read_landed(root).unwrap();
    assert_eq!(landed.len(), 4);
    let mut saw_in_reply_to = false;
    let mut saw_references_only = false;
    let mut saw_forwarded = false;
    for rec in &landed {
        let p = root
            .join("chronicle")
            .join(&rec.segment_rel)
            .join("conversation_transcript.jsonl");
        let text = std::fs::read_to_string(&p).unwrap();
        let row: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
        let row_map = row.as_object().unwrap();

        // Cross-crate roundtrip check: every landed exchange must read cleanly
        // through solstone-core-format's conversation reader and search indexer.
        let item = solstone_core_format::content::conversation::conversation_item(
            root,
            &rec.segment_rel,
            "conversation_transcript.jsonl",
        )
        .expect("conversation_item must recognize landed import.email segment");
        assert_eq!(item.source, Some("email"));
        assert_eq!(item.content_kind, Some("messages"));
        assert!(
            item.first_sent_at.is_some() && item.last_sent_at.is_some(),
            "header line must carry first_sent_at and last_sent_at"
        );
        let projected = solstone_core_format::content::conversation::message_fields(
            "import.email",
            "conversation_transcript.jsonl",
            0,
            row_map,
            false,
        )
        .expect("message_fields must project landed email row");
        let indexed = solstone_core_format::content::conversation::indexed_message(row_map)
            .expect("indexed_message must index landed email row");

        if rec.key == "gm:1002" {
            saw_in_reply_to = true;
            assert!(
                row["text"]
                    .as_str()
                    .unwrap()
                    .contains("Yes, the afternoon works.")
            );
            assert!(!row["text"].as_str().unwrap().contains("Could we meet"));
            assert_eq!(row["email"]["parent_match_rule"], "in_reply_to");
            assert_eq!(projected["reply_to"]["author"], "Casey");
            assert!(
                projected["reply_to"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Thursday afternoon")
            );
            assert_eq!(projected["attachments"][0]["name"], "agenda.pdf");
            assert_eq!(projected["attachments"][1]["name"], "site-photo.jpg");
            assert_eq!(row["attachments"][1]["inline"], true);
            assert_eq!(
                projected["attachments"][1],
                serde_json::json!({"name":"site-photo.jpg"})
            );
            assert!(indexed.contains("Thursday afternoon"));
            assert!(indexed.contains("Yes, the afternoon works."));
        }
        if rec.key == "gm:1005" {
            saw_references_only = true;
            assert_eq!(row["email"]["parent_match_rule"], "references_only");
            assert_eq!(projected["reply_to"]["author"], "Casey");
        }
        if rec.key == "gm:1004" {
            saw_forwarded = true;
            assert!(row["text"].as_str().unwrap().contains("Looping you in"));
            assert_eq!(projected["forwarded"][0]["subject"], "Room schedule");
            assert!(
                projected["forwarded"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Room seven is free")
            );
            assert!(indexed.contains("Room seven is free"));
        }
    }
    assert!(saw_in_reply_to && saw_references_only && saw_forwarded);

    // 3. Delete one synced exchange in the journal (tombstone) -> accounting moves to removed_by_owner
    //    and re-sync NEVER brings it back (AC 17).
    let deleted_rec = landed.iter().find(|r| r.key == "gm:1003").unwrap().clone();
    std::fs::write(
        root.join("chronicle")
            .join(&deleted_rec.segment_rel)
            .join("tombstone.json"),
        b"{}",
    )
    .unwrap();
    let st_after_del = sync::status_snapshot(root, &tz).unwrap();
    assert_eq!(st_after_del.accounting.in_journal, 3);
    assert_eq!(st_after_del.accounting.removed_by_owner, 1);
    assert_eq!(st_after_del.accounting.outside_choice, 1);
    assert_eq!(st_after_del.accounting.left_out.total, 4);
    assert_eq!(
        st_after_del.accounting.sent_total,
        st_after_del.accounting.in_journal
            + st_after_del.accounting.removed_by_owner
            + st_after_del.accounting.outside_choice
            + st_after_del.accounting.left_out.total
    );

    // 4. Widen choice from 30d to `all`, untick `forwards` (future-only; landed forward stays!),
    //    and delete gm:1002 from the mock server to verify AC 18 (deleted-in-mailbox exchange stays).
    sync::save_choice(
        root,
        RangePreset::All,
        vec![MessageType::Replies, MessageType::NewMessages],
        now,
        &tz,
    )
    .unwrap();
    let mut reduced_msgs = msgs.clone();
    reduced_msgs.retain(|m| m.uid != 2);
    // Also append a brand-new sent message (UID 11) to verify incremental live sync (AC 16).
    let body11 = "Fresh message sent after setup.\r\n";
    reduced_msgs.push(MockMessage {
        uid: 11,
        gm_msgid: "1011".into(),
        gm_thrid: "t11".into(),
        labels: vec!["\\Sent".into()],
        flags: vec!["\\Seen".into()],
        internal_date: "25-Nov-2026 11:45:00 -0700".into(),
        bodystructure: plain_bodystructure(body11.len()),
        raw_headers: "Message-ID: <fresh-11@example.com>\r\nFrom: Maya Chen <maya@example.com>\r\nTo: Sam <sam@example.com>\r\nDate: Wed, 25 Nov 2026 11:45:00 -0700\r\nSubject: Lunch today\r\n\r\n".into(),
        parts: vec![("1".into(), "Content-Type: text/plain; charset=utf-8\r\n".into(), body11.into())],
    });
    let (stream, handle) = mock_server(reduced_msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let widened = sync::run_sync(root, &mut client, now, &tz, SyncOptions::default()).unwrap();
    // Should land ONLY gm:1010 (older) + gm:1011 (new), NOT gm:1003 (deleted by owner) or already-landed ones!
    assert_eq!(widened.landed_messages, 2);
    client.logout().unwrap();
    let _ = handle.join();

    // Verify gm:1002 (deleted from server) and gm:1004 (forward landed before unticking) BOTH remain live!
    let landed_after = store::read_landed(root).unwrap();
    let live = store::live_landed_map(root, &landed_after);
    assert_eq!(live.get("gm:1002"), Some(&true));
    assert_eq!(live.get("gm:1004"), Some(&true));
    assert_eq!(live.get("gm:1003"), Some(&false));
    assert_eq!(live.get("gm:1010"), Some(&true));
    assert_eq!(live.get("gm:1011"), Some(&true));

    // 5. Cache delete & rebuild produces identical decisions (AC 14).
    let decisions_before = store::read_decisions(root).unwrap();
    std::fs::remove_file(store::header_cache_path(root)).unwrap();
    let (stream, handle) = mock_server(reduced_msgs).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::run_header_scan(root, &mut client, now, &tz, false).unwrap();
    client.logout().unwrap();
    let _ = handle.join();
    let decisions_after = store::read_decisions(root).unwrap();
    assert_eq!(decisions_before, decisions_after);

    // 6. Disconnect removes credentials while keeping exchanges and landed ledger.
    sync::disconnect(root, now).unwrap();
    assert!(store::read_credentials(root).unwrap().is_none());
    let disconnected = sync::status_snapshot(root, &tz).unwrap();
    assert_eq!(disconnected.state, "off");
    assert_eq!(disconnected.phase, "disconnected");
    assert_eq!(disconnected.accounting.in_journal, 5);
}

#[test]
fn versioned_rereach_adds_newly_qualified_skipped_mail_without_touching_landed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let tz = denver_tz();
    let now = now_ts();
    let msgs = sample_mailbox_messages();

    // Initial connect, scan, choose all, sync.
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd1234efgh5678",
        &mut client,
        now,
    )
    .unwrap();
    sync::run_header_scan(root, &mut client, now, &tz, false).unwrap();
    sync::save_choice(root, RangePreset::All, MessageType::ALL.to_vec(), now, &tz).unwrap();
    sync::run_sync(root, &mut client, now, &tz, SyncOptions::default()).unwrap();
    client.logout().unwrap();
    let _ = handle.join();

    // Simulate an older release (rules_version=RULES_VERSION) having left out UID 10 (gm:1010)
    // and never having landed it, while gm:1002 is already landed at that version.
    let current_rules = crate::select::RULES_VERSION;
    let next_rules = current_rules + 1;
    let mut decisions = store::read_decisions(root).unwrap();
    for d in &mut decisions {
        if d.key == "gm:1010" {
            d.outcome = "left_out".into();
            d.message_type = None;
            d.reason = Some("legacy_skip".into());
            d.left_out_bucket = Some(LeftOutBucket::AutomaticReplies);
            d.rules_version = current_rules;
        }
    }
    store::write_decisions(root, &decisions).unwrap();
    let mut landed = store::read_landed(root).unwrap();
    let gm1002_before = landed.iter().find(|r| r.key == "gm:1002").unwrap().clone();
    landed.retain(|r| r.key != "gm:1010");
    store::write_landed(root, &landed).unwrap();

    // At current stale_before (no upgrade), gm:1010 remains skipped.
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let no_upgrade = sync::run_sync(root, &mut client, now, &tz, SyncOptions::default()).unwrap();
    assert_eq!(no_upgrade.landed_messages, 0);
    client.logout().unwrap();
    let _ = handle.join();

    // Release bumps rules_version & stale_before_version -> gm:1010 re-evaluated and lands,
    // while gm:1002 is untouched.
    let (stream, handle) = mock_server(msgs).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let upgraded = sync::run_sync(
        root,
        &mut client,
        now,
        &tz,
        SyncOptions {
            rules_version: Some(next_rules),
            stale_before_version: Some(next_rules),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upgraded.landed_messages, 1);
    client.logout().unwrap();
    let _ = handle.join();

    let landed_after = store::read_landed(root).unwrap();
    assert!(landed_after.iter().any(|r| r.key == "gm:1010"));
    let gm1002_after = landed_after.iter().find(|r| r.key == "gm:1002").unwrap();
    assert_eq!(gm1002_before, *gm1002_after);
}

#[test]
fn cap_text_follows_paragraph_line_word_hard_order_and_folder_cap_detection() {
    let para = format!("{}\n\n{}", "a".repeat(15_000), "b".repeat(10_000));
    let c = cap_text(&para, CAP_CHARS);
    assert!(c.truncated);
    assert_eq!(c.cut_at, Some("paragraph"));
    assert_eq!(c.text.len(), 15_000);
    assert_eq!(c.cut_chars, para.chars().count() - 15_000);

    let lines = format!("{}\n{}", "a".repeat(15_000), "b".repeat(10_000));
    let c = cap_text(&lines, CAP_CHARS);
    assert_eq!(c.cut_at, Some("line"));

    let words = format!("{} {}", "a".repeat(15_000), "b".repeat(10_000));
    let c = cap_text(&words, CAP_CHARS);
    assert_eq!(c.cut_at, Some("word"));

    let solid = "x".repeat(25_000);
    let c = cap_text(&solid, CAP_CHARS);
    assert_eq!(c.cut_at, Some("hard"));
    assert_eq!(c.text.len(), CAP_CHARS);
    assert_eq!(c.cut_chars, 5_000);

    assert_eq!(detect_folder_cap(1_000), Some(1_000));
    assert_eq!(detect_folder_cap(10_000), Some(10_000));
    assert_eq!(detect_folder_cap(10_001), None);

    assert_eq!(owner_fallback_text(1, 0, false), "sent with an attachment");
    assert_eq!(owner_fallback_text(2, 0, false), "sent with attachments");
    assert_eq!(owner_fallback_text(0, 1, false), "sent with an image");
    assert_eq!(owner_fallback_text(0, 3, false), "sent with images");
    assert_eq!(
        owner_fallback_text(1, 1, false),
        "sent with an attachment and an image"
    );
    assert_eq!(
        owner_fallback_text(1, 2, false),
        "sent with an attachment and images"
    );
    assert_eq!(
        owner_fallback_text(2, 1, false),
        "sent with attachments and an image"
    );
    assert_eq!(
        owner_fallback_text(2, 2, false),
        "sent with attachments and images"
    );
    assert_eq!(owner_fallback_text(0, 0, true), "forwarded without comment");
    assert_eq!(owner_fallback_text(0, 0, false), "sent with no text");
}

#[test]
fn synthetic_labelled_extraction_corpus_loses_zero_owner_spans() {
    struct Case {
        name: &'static str,
        plain: Option<&'static str>,
        flowed: Option<bool>,
        html: Option<&'static str>,
        parent: Option<&'static str>,
        is_reply: bool,
        owner_spans: &'static [&'static str],
        quoted_spans: &'static [&'static str],
        expect_uncertain: bool,
        expect_forwarded: Option<&'static str>,
    }

    let sigs = BTreeSet::from([loose("Maya Chen\nLead Architect")]);
    let cases = [
        Case {
            name: "gmail_standard_reply",
            plain: Some(
                "Sounds good, let's lock Tuesday at 3.\n\nOn Mon, Nov 16, 2026 at 10:00 AM Casey <casey@example.com> wrote:\n> Can we lock Tuesday at 3 for the workshop?\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Can we lock Tuesday at 3 for the workshop?"),
            is_reply: true,
            owner_spans: &["Sounds good, let's lock Tuesday at 3."],
            quoted_spans: &["Can we lock Tuesday at 3 for the workshop?"],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "wrapped_attribution_two_lines",
            plain: Some(
                "I'll bring the samples.\n\nOn Tue, Nov 17, 2026 at 2:15 PM Alex Rivera <\nalex@example.com> wrote:\n> Please remember to bring the tile samples tomorrow.\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Please remember to bring the tile samples tomorrow."),
            is_reply: true,
            owner_spans: &["I'll bring the samples."],
            quoted_spans: &["Please remember to bring the tile samples tomorrow."],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "inline_interleaved_reply_keeps_owner_between_quotes",
            plain: Some(
                "On Mon, Nov 16, 2026 at 9:00 AM Sam <sam@example.com> wrote:\n> Should we ship the draft tonight?\nYes, push it once the tests turn green.\n\n> And what about the blog post?\nHold the blog post until Thursday morning.\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Should we ship the draft tonight?\n\nAnd what about the blog post?"),
            is_reply: true,
            owner_spans: &[
                "Yes, push it once the tests turn green.",
                "Hold the blog post until Thursday morning.",
            ],
            quoted_spans: &[
                "Should we ship the draft tonight?",
                "And what about the blog post?",
            ],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "parent_veto_protects_owner_quoting_third_party",
            plain: Some(
                "Here is what the lease says:\n\n> Tenant shall maintain quiet hours after ten pm.\n\nWe should ask them to clarify that clause.\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Did you get a chance to read the new office lease?"),
            is_reply: true,
            owner_spans: &[
                "Here is what the lease says:",
                "Tenant shall maintain quiet hours after ten pm.",
                "We should ask them to clarify that clause.",
            ],
            quoted_spans: &[],
            expect_uncertain: true,
            expect_forwarded: None,
        },
        Case {
            name: "outlook_reply_with_bottom_owner_note_and_parent",
            plain: Some(
                "Got it, thanks!\n\n________________________________\nFrom: Jordan <jordan@example.com>\nSent: Monday, November 16, 2026 9:00 AM\nTo: Maya <maya@example.com>\nSubject: Budget\n\nPlease review the attached quarterly numbers before Friday.\n\nActually one more thing: let's add travel.\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Please review the attached quarterly numbers before Friday."),
            is_reply: true,
            owner_spans: &[
                "Got it, thanks!",
                "Actually one more thing: let's add travel.",
            ],
            quoted_spans: &["Please review the attached quarterly numbers before Friday."],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "apple_mail_forward_keeps_comment_and_forwarded_body",
            plain: Some(
                "Sharing this itinerary for next week.\n\nBegin forwarded message:\n\nFrom: Airline <flights@example.com>\nDate: November 15, 2026 at 8:00 AM\nSubject: Flight 402 confirmed\n\nDepart Denver 08:15, arrive Seattle 10:05.\n",
            ),
            flowed: None,
            html: None,
            parent: None,
            is_reply: false,
            owner_spans: &["Sharing this itinerary for next week."],
            quoted_spans: &[],
            expect_uncertain: false,
            expect_forwarded: Some("Depart Denver 08:15, arrive Seattle 10:05."),
        },
        Case {
            name: "recurring_sig_and_mobile_footer_stripped",
            plain: Some(
                "Leaving for the airport now.\n\n-- \nMaya Chen\nLead Architect\n\nSent from my iPhone\n",
            ),
            flowed: None,
            html: None,
            parent: None,
            is_reply: false,
            owner_spans: &["Leaving for the airport now."],
            quoted_spans: &["Maya Chen"],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "owner_saying_here_is_what_i_wrote_is_not_attribution",
            plain: Some("Here is what I wrote:\n\nWe will open the studio at nine sharp.\n"),
            flowed: None,
            html: None,
            parent: None,
            is_reply: false,
            owner_spans: &[
                "Here is what I wrote:",
                "We will open the studio at nine sharp.",
            ],
            quoted_spans: &[],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "html_only_gmail_quote_and_signature_stripped",
            plain: None,
            flowed: None,
            html: Some(
                "<div>Count me in for Saturday morning.</div><div class=\"gmail_signature\">Maya Chen</div><div class=\"gmail_quote\"><div class=\"gmail_attr\">On Wed, Nov 18, 2026 at 9:00 AM Lee &lt;lee@example.com&gt; wrote:</div><blockquote class=\"gmail_quote\">Are you joining the trail run on Saturday?</blockquote></div>",
            ),
            parent: Some("Are you joining the trail run on Saturday?"),
            is_reply: true,
            owner_spans: &["Count me in for Saturday morning."],
            quoted_spans: &["Are you joining the trail run on Saturday?"],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "rfc3676_flowed_unwrapping",
            plain: Some(
                "We decided to keep the timber beams exposed \nacross the whole ceiling.\n",
            ),
            flowed: Some(false),
            html: None,
            parent: None,
            is_reply: false,
            owner_spans: &["We decided to keep the timber beams exposed across the whole ceiling."],
            quoted_spans: &[],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "german_attribution_reply",
            plain: Some(
                "Klingt perfekt, bis Freitag.\n\nAm 18.11.2026 um 14:00 schrieb Lukas <lukas@example.de>:\n> Passt dir Freitag um zehn Uhr?\n",
            ),
            flowed: None,
            html: None,
            parent: Some("Passt dir Freitag um zehn Uhr?"),
            is_reply: true,
            owner_spans: &["Klingt perfekt, bis Freitag."],
            quoted_spans: &["Passt dir Freitag um zehn Uhr?"],
            expect_uncertain: false,
            expect_forwarded: None,
        },
        Case {
            name: "japanese_unprefixed_attribution",
            plain: Some(
                "承知しました、明日送ります。\n\n2026年11月18日 15:30 佐藤 <sato@example.jp>:\n> 仕様書の最新版を共有していただけますか？\n",
            ),
            flowed: None,
            html: None,
            parent: Some("仕様書の最新版を共有していただけますか？"),
            is_reply: true,
            owner_spans: &["承知しました、明日送ります。"],
            quoted_spans: &["仕様書の最新版を共有していただけますか？"],
            expect_uncertain: false,
            expect_forwarded: None,
        },
    ];

    let mut owner_total = 0usize;
    let mut owner_lost = 0usize;
    let mut quoted_total = 0usize;
    let mut quoted_left_in = 0usize;

    for c in &cases {
        let ev = Evidence {
            parent: c.parent,
            recurring_signatures: &sigs,
        };
        let ex = extract::extract(c.plain, c.flowed, c.html, c.is_reply, &ev);
        let hay = loose(&ex.text);
        for span in c.owner_spans {
            owner_total += 1;
            if !hay.contains(&loose(span)) {
                owner_lost += 1;
                eprintln!(
                    "case {} lost owner span {:?}; extracted: {:?}",
                    c.name, span, ex.text
                );
            }
        }
        for span in c.quoted_spans {
            quoted_total += 1;
            if hay.contains(&loose(span)) {
                quoted_left_in += 1;
            }
        }
        if c.expect_uncertain {
            assert!(
                !ex.uncertain_spans.is_empty(),
                "case {} expected uncertain_spans",
                c.name
            );
            for sp in &ex.uncertain_spans {
                assert!(sp.start < sp.end && sp.end <= ex.text.len());
                assert!(ex.text.is_char_boundary(sp.start) && ex.text.is_char_boundary(sp.end));
            }
        }
        if let Some(want_fwd) = c.expect_forwarded {
            let got_fwd = ex.forwarded.first().map(|f| f.text.as_str()).unwrap_or("");
            assert!(
                got_fwd.contains(want_fwd),
                "case {} expected forwarded {:?}, got {:?}",
                c.name,
                want_fwd,
                got_fwd
            );
        }
    }
    assert_eq!(owner_lost, 0, "0 of {owner_total} owner spans may be lost");
    assert_eq!(
        quoted_left_in, 0,
        "0 of {quoted_total} two-signal quoted spans should remain"
    );
}

#[test]
fn accounting_reconciles_every_bucket_identically() {
    let mut decisions = Vec::new();
    let mut live = std::collections::BTreeMap::new();
    for i in 0..5 {
        let key = format!("gm:{i}");
        decisions.push(crate::select::Decision {
            key: key.clone(),
            reference: crate::model::Reference::default(),
            outcome: "selected".into(),
            message_type: Some(MessageType::Replies),
            reason: None,
            left_out_bucket: None,
            parent: crate::select::ParentOutcome {
                outcome: "certain".into(),
                parent_match_rule: Some(ParentMatchRule::InReplyTo),
                ..Default::default()
            },
            sent_ms: Some(1_700_000_000_000),
            rules_version: 1,
        });
        if i < 2 {
            live.insert(key, true);
        } else if i == 2 {
            live.insert(key, false);
        }
    }
    decisions.push(crate::select::Decision {
        key: "gm:draft".into(),
        reference: crate::model::Reference::default(),
        outcome: "left_out".into(),
        message_type: None,
        reason: Some("draft".into()),
        left_out_bucket: Some(LeftOutBucket::Drafts),
        parent: Default::default(),
        sent_ms: Some(1_700_000_000_000),
        rules_version: 1,
    });
    let acc = compute_accounting(&decisions, &live, None);
    assert_eq!(acc.sent_total, 6);
    assert_eq!(acc.in_journal, 2);
    assert_eq!(acc.removed_by_owner, 1);
    assert_eq!(acc.outside_choice, 2);
    assert_eq!(acc.left_out.total, 1);
    assert_eq!(
        acc.sent_total,
        acc.in_journal + acc.removed_by_owner + acc.outside_choice + acc.left_out.total
    );
}

#[test]
fn from_alias_recovers_unlabelled_sent_mail_without_trash_junk_or_echo_dupes() {
    use crate::model::{Addr, Header, Location, Part};
    use crate::select::{RULES_VERSION, evaluate_decisions};
    use std::collections::HashMap;

    let make_hdr =
        |key: &str, uid: u32, from: &str, labels: &[&str], msg_id: Option<&str>, subject: &str| {
            Header {
                key: key.to_owned(),
                location: Location {
                    mailbox: "[Gmail]/All Mail".into(),
                    uid_validity: 1,
                    uid,
                },
                labels: labels.iter().map(|s| (*s).to_owned()).collect(),
                message_id: msg_id.map(str::to_owned),
                from: vec![Addr {
                    name: Some("Maya".into()),
                    address: Some(from.to_owned()),
                }],
                to: vec![Addr {
                    name: Some("Sam".into()),
                    address: Some("sam@example.com".into()),
                }],
                date_ms: Some(1_760_000_000_000 + i64::from(uid) * 1_000),
                subject: Some(subject.to_owned()),
                tree: Part {
                    path: vec![1],
                    mime: "text/plain".into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        };

    let aliases = vec!["maya@example.com".to_owned(), "maya@work.org".to_owned()];
    let headers = vec![
        // Standard \Sent message
        make_hdr(
            "gm:1",
            1,
            "maya@example.com",
            &["\\Sent"],
            Some("<m1@example.com>"),
            "Normal sent",
        ),
        // Mailing-list echo without \Sent sharing Message-ID with gm:1 -> must dedupe
        make_hdr(
            "gm:2",
            2,
            "maya@example.com",
            &["\\Inbox"],
            Some("<m1@example.com>"),
            "Normal sent",
        ),
        // Unlabelled owner-from message (primary address) -> must recover
        make_hdr(
            "gm:3",
            3,
            "maya@example.com",
            &[],
            Some("<m3@example.com>"),
            "Archived sent",
        ),
        // Unlabelled owner-from message (secondary alias) -> must recover
        make_hdr(
            "gm:4",
            4,
            "maya@work.org",
            &[],
            Some("<m4@work.org>"),
            "Alias sent",
        ),
        // Duplicate unlabelled copy sharing Message-ID with gm:4 -> must dedupe
        make_hdr(
            "gm:5",
            5,
            "maya@work.org",
            &[],
            Some("<m4@work.org>"),
            "Alias sent",
        ),
        // Unlabelled owner-from in \Trash -> must exclude
        make_hdr(
            "gm:6",
            6,
            "maya@example.com",
            &["\\Trash"],
            Some("<m6@example.com>"),
            "Trashed",
        ),
        // Unlabelled owner-from in \Junk -> must exclude
        make_hdr(
            "gm:7",
            7,
            "maya@example.com",
            &["\\Junk"],
            Some("<m7@example.com>"),
            "Junked",
        ),
        // Non-owner inbound message -> must exclude
        make_hdr(
            "gm:8",
            8,
            "sam@example.com",
            &["\\Inbox"],
            Some("<m8@example.com>"),
            "Inbound",
        ),
        // Owner draft -> left_out (Drafts), not selected
        make_hdr(
            "gm:9",
            9,
            "maya@example.com",
            &["\\Draft"],
            Some("<m9@example.com>"),
            "Draft",
        ),
    ];

    let decisions = evaluate_decisions(
        "maya@example.com",
        &aliases,
        &headers,
        &HashMap::new(),
        &BTreeSet::new(),
        RULES_VERSION,
        RULES_VERSION,
    );
    let selected_keys: BTreeSet<&str> = decisions
        .iter()
        .filter(|d| d.outcome == "selected")
        .map(|d| d.key.as_str())
        .collect();
    assert_eq!(
        selected_keys,
        BTreeSet::from(["gm:1", "gm:3", "gm:4"]),
        "expected standard sent + unlabelled primary + unlabelled alias without dupes/trash/junk"
    );
    let draft = decisions.iter().find(|d| d.key == "gm:9").expect("draft");
    assert_eq!(draft.outcome, "left_out");
    assert_eq!(draft.left_out_bucket, Some(LeftOutBucket::Drafts));
    assert!(
        !decisions
            .iter()
            .any(|d| matches!(d.key.as_str(), "gm:2" | "gm:5" | "gm:6" | "gm:7" | "gm:8")),
        "echo dupes, trash/junk, and inbound mail must not enter decisions"
    );
}

#[test]
fn reconnecting_imap_source_recovers_transient_drops_and_stops_on_auth_error() {
    use crate::imap::{MailboxInfo, ReconnectingImapSource, WireAudit};
    use crate::model::{Header, Part};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FlakySource {
        fail_scan: Arc<Mutex<usize>>,
        fail_fetch: Arc<Mutex<usize>>,
        fail_auth: bool,
        uid_validity: u32,
        inner: Option<Box<dyn ImapSource + Send>>,
    }

    impl ImapSource for FlakySource {
        fn mailbox_info(&mut self) -> crate::error::Result<MailboxInfo> {
            if self.fail_auth {
                return Err(EmailError::AuthFailed);
            }
            let mut mb = match self.inner.as_mut() {
                Some(inner) => inner.mailbox_info()?,
                None => MailboxInfo {
                    name: "[Gmail]/All Mail".to_owned(),
                    uid_validity: self.uid_validity,
                    uid_next: 10,
                    exists: 1,
                },
            };
            mb.uid_validity = self.uid_validity;
            Ok(mb)
        }

        fn scan_page(
            &mut self,
            mailbox: &MailboxInfo,
            lo: u32,
            hi: u32,
        ) -> crate::error::Result<Vec<Header>> {
            let mut rem = self.fail_scan.lock().unwrap();
            if *rem > 0 {
                *rem -= 1;
                return Err(EmailError::Unreachable("simulated TLS EOF".into()));
            }
            drop(rem);
            self.inner.as_mut().unwrap().scan_page(mailbox, lo, hi)
        }

        fn fetch_texts(
            &mut self,
            reqs: &[(&Header, &Part)],
        ) -> crate::error::Result<Vec<Option<String>>> {
            let mut rem = self.fail_fetch.lock().unwrap();
            if *rem > 0 {
                *rem -= 1;
                return Err(EmailError::Protocol("simulated BYE timeout".into()));
            }
            drop(rem);
            self.inner.as_mut().unwrap().fetch_texts(reqs)
        }

        fn wire_audit(&self) -> WireAudit {
            self.inner
                .as_ref()
                .map_or_else(WireAudit::default, |s| s.wire_audit())
        }

        fn logout(&mut self) -> crate::error::Result<()> {
            if let Some(s) = self.inner.as_mut() {
                s.logout()?;
            }
            Ok(())
        }
    }

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let msgs = sample_mailbox_messages();
    let tz = denver_tz();
    let now = now_ts();

    let opens = Arc::new(Mutex::new(0usize));
    let fail_scan = Arc::new(Mutex::new(1usize));
    let fail_fetch = Arc::new(Mutex::new(1usize));
    let handles = Arc::new(Mutex::new(Vec::new()));

    let open_session = {
        let msgs = msgs.clone();
        let opens = Arc::clone(&opens);
        let fail_scan = Arc::clone(&fail_scan);
        let fail_fetch = Arc::clone(&fail_fetch);
        let handles = Arc::clone(&handles);
        move || -> crate::error::Result<Box<dyn ImapSource + Send>> {
            *opens.lock().unwrap() += 1;
            let (stream, handle) = mock_server(msgs.clone()).spawn();
            handles.lock().unwrap().push(handle);
            let client = GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678")?;
            let uid_validity = 4242;
            Ok(Box::new(FlakySource {
                fail_scan: Arc::clone(&fail_scan),
                fail_fetch: Arc::clone(&fail_fetch),
                fail_auth: false,
                uid_validity,
                inner: Some(Box::new(client)),
            }))
        }
    };

    let mut source = ReconnectingImapSource::new(open_session, 3, Duration::ZERO);
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd 1234 efgh 5678",
        &mut source,
        now,
    )
    .unwrap();
    let scan = sync::run_header_scan(root, &mut source, now, &tz, false).unwrap();
    assert!(scan.complete);
    assert_eq!(scan.eligible_sent, 5);

    sync::save_choice(root, RangePreset::All, MessageType::ALL.to_vec(), now, &tz).unwrap();
    let report = sync::run_sync(root, &mut source, now, &tz, SyncOptions::default()).unwrap();
    assert_eq!(report.landed_messages, 5);
    assert_eq!(
        *opens.lock().unwrap(),
        3,
        "initial open + 1 reconnect on scan drop + 1 reconnect on fetch drop"
    );
    assert!(report.wire.commands > 0);
    source.logout().unwrap();
    for h in handles.lock().unwrap().drain(..) {
        let _ = h.join();
    }

    // Permanent AuthFailed error must never be retried.
    let auth_opens = Arc::new(Mutex::new(0usize));
    let auth_opens_c = Arc::clone(&auth_opens);
    let mut auth_src = ReconnectingImapSource::new(
        move || -> crate::error::Result<Box<dyn ImapSource + Send>> {
            *auth_opens_c.lock().unwrap() += 1;
            Err(EmailError::AuthFailed)
        },
        3,
        Duration::ZERO,
    );
    let err = auth_src.mailbox_info().unwrap_err();
    assert!(matches!(err, EmailError::AuthFailed));
    assert_eq!(*auth_opens.lock().unwrap(), 1);

    // UIDVALIDITY change across reconnect must fail immediately without burning retries.
    let uv_opens = Arc::new(Mutex::new(0usize));
    let uv_opens_c = Arc::clone(&uv_opens);
    let mut uv_src = ReconnectingImapSource::new(
        move || -> crate::error::Result<Box<dyn ImapSource + Send>> {
            let mut n = uv_opens_c.lock().unwrap();
            *n += 1;
            let attempt = *n;
            Ok(Box::new(FlakySource {
                fail_scan: Arc::new(Mutex::new(usize::from(attempt == 1))),
                fail_fetch: Arc::new(Mutex::new(0)),
                fail_auth: false,
                uid_validity: if attempt == 1 { 4242 } else { 9999 },
                inner: None,
            }))
        },
        3,
        Duration::ZERO,
    );
    let mb = MailboxInfo {
        name: "[Gmail]/All Mail".to_owned(),
        uid_validity: 4242,
        uid_next: 10,
        exists: 1,
    };
    let uv_err = uv_src.scan_page(&mb, 1, 10).unwrap_err();
    assert!(matches!(uv_err, EmailError::Protocol(ref m) if m.contains("UIDVALIDITY")));
    assert_eq!(
        *uv_opens.lock().unwrap(),
        2,
        "UIDVALIDITY mismatch must stop on the first reconnect instead of burning retries"
    );
}

#[test]
fn fetch_error_clears_fetching_phase_and_stale_worker_state_reconciles() {
    use crate::imap::{MailboxInfo, WireAudit};
    use crate::model::{Header, Part};

    struct FailOnFetch(Box<dyn ImapSource + Send>);
    impl ImapSource for FailOnFetch {
        fn mailbox_info(&mut self) -> crate::error::Result<MailboxInfo> {
            self.0.mailbox_info()
        }
        fn scan_page(
            &mut self,
            mailbox: &MailboxInfo,
            lo: u32,
            hi: u32,
        ) -> crate::error::Result<Vec<Header>> {
            self.0.scan_page(mailbox, lo, hi)
        }
        fn fetch_texts(
            &mut self,
            _reqs: &[(&Header, &Part)],
        ) -> crate::error::Result<Vec<Option<String>>> {
            Err(EmailError::Unreachable("network dropped".into()))
        }
        fn wire_audit(&self) -> WireAudit {
            self.0.wire_audit()
        }
        fn logout(&mut self) -> crate::error::Result<()> {
            self.0.logout()
        }
    }

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let msgs = sample_mailbox_messages();
    let tz = denver_tz();
    let now = now_ts();

    let (stream, handle) = mock_server(msgs).spawn();
    let client = GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let mut failing = FailOnFetch(Box::new(client));
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd 1234 efgh 5678",
        &mut failing,
        now,
    )
    .unwrap();
    sync::run_header_scan(root, &mut failing, now, &tz, false).unwrap();
    sync::save_choice(root, RangePreset::All, MessageType::ALL.to_vec(), now, &tz).unwrap();

    let err = sync::run_sync(root, &mut failing, now, &tz, SyncOptions::default()).unwrap_err();
    assert!(matches!(err, EmailError::Unreachable(_)));
    let _ = failing.logout();
    let _ = handle.join();

    // After transient fetch failure, persisted state must NOT remain stuck in "fetching".
    let state = store::read_state(root).unwrap();
    assert!(!state.fetching_active);
    assert_eq!(state.phase, "steady");

    // Simulate a process crash leaving stale "fetching" or "scanning" in state.json.
    let mut crashed_fetch = state.clone();
    crashed_fetch.fetching_active = true;
    crashed_fetch.phase = "fetching".to_owned();
    store::write_state(root, &crashed_fetch).unwrap();

    let dead_fetch = sync::status_snapshot_with_activity(root, &tz, false).unwrap();
    assert_eq!(dead_fetch.phase, "steady");
    assert!(!dead_fetch.syncing);
    assert!(!dead_fetch.sync.active);

    let live_fetch = sync::status_snapshot_with_activity(root, &tz, true).unwrap();
    assert_eq!(live_fetch.phase, "fetching");
    assert!(live_fetch.syncing);
    assert!(live_fetch.sync.active);

    let mut crashed_scan = state;
    crashed_scan.scan_complete = false;
    crashed_scan.scan_paused = false;
    crashed_scan.phase = "scanning".to_owned();
    store::write_state(root, &crashed_scan).unwrap();

    let dead_scan = sync::status_snapshot_with_activity(root, &tz, false).unwrap();
    assert_eq!(dead_scan.phase, "scan_paused");
    assert!(dead_scan.scan.paused);
    assert!(!dead_scan.syncing);

    let live_scan = sync::status_snapshot_with_activity(root, &tz, true).unwrap();
    assert_eq!(live_scan.phase, "scanning");
    assert!(!live_scan.scan.paused);
    assert!(live_scan.syncing);
}

#[test]
fn steady_noop_scan_skips_decision_rewrite_until_rules_or_uid_changes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let msgs = sample_mailbox_messages();
    let tz = denver_tz();
    let now = now_ts();

    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd 1234 efgh 5678",
        &mut client,
        now,
    )
    .unwrap();
    sync::run_header_scan(root, &mut client, now, &tz, false).unwrap();
    client.logout().unwrap();
    let _ = handle.join();

    // Marker line appended to decisions.jsonl: a no-op tick must not rewrite the file.
    let decisions_file = store::decisions_path(root);
    let orig = std::fs::read_to_string(&decisions_file).unwrap();
    let with_blank = format!("{orig}\n");
    std::fs::write(&decisions_file, &with_blank).unwrap();

    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let later = Timestamp::from_second(now.as_second() + 3600).unwrap();
    let noop = sync::run_header_scan(root, &mut client, later, &tz, false).unwrap();
    assert!(noop.complete);
    assert_eq!(noop.eligible_sent, 5);
    client.logout().unwrap();
    let _ = handle.join();
    assert_eq!(
        std::fs::read_to_string(&decisions_file).unwrap(),
        with_blank,
        "0-new-mail tick at current rules_version must skip rewriting decisions.jsonl"
    );

    // Simulating an upgrade from an earlier rules_version must force re-evaluation.
    let mut st = store::read_state(root).unwrap();
    st.last_rules_version = 1;
    store::write_state(root, &st).unwrap();
    let (stream, handle) = mock_server(msgs).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::run_header_scan(root, &mut client, later, &tz, false).unwrap();
    client.logout().unwrap();
    let _ = handle.join();
    assert_eq!(
        std::fs::read_to_string(&decisions_file).unwrap(),
        orig,
        "older last_rules_version must trigger full decision re-evaluation"
    );
    assert_eq!(
        store::read_state(root).unwrap().last_rules_version,
        crate::select::RULES_VERSION
    );
}

#[test]
fn upgrading_recovers_selected_mixed_calendar_mail_and_legacy_opaque_filenames_without_body_reads()
{
    let d = TempDir::new().unwrap();
    let root = d.path();
    let mut mail = sample_mailbox_messages();
    let mixed = concat!(
        r#"(("TEXT" "PLAIN" NIL NIL NIL "7BIT" 40 1 NIL NIL NIL)"#,
        r#"("TEXT" "CALENDAR" ( "NAME" "event.ics") NIL NIL "7BIT" 90 4 NIL ("ATTACHMENT" ("FILENAME" "event.ics")) NIL)"#,
        r#"("APPLICATION" "OCTET-STREAM" NIL NIL NIL "BASE64" 40 NIL ("ATTACHMENT" ("FILENAME*" "utf-8''caf%C3%A9.pdf")) NIL) "MIXED" NIL NIL NIL)"#
    );
    // The fixture's owner note was rejected by the old presence-of-ICS rule.
    mail.iter_mut().find(|m| m.uid == 6).unwrap().bodystructure = mixed.into();
    let (stream, handle) = mock_server(mail).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd1234efgh5678",
        &mut client,
        now_ts(),
    )
    .unwrap();
    sync::run_header_scan(root, &mut client, now_ts(), &denver_tz(), false).unwrap();

    let db = rusqlite::Connection::open(store::header_cache_path(root)).unwrap();
    db.execute("UPDATE messages SET json=json_remove(json_set(json,'$.tree.children[2].filename',null),'$.attachment_names_checked') WHERE key='gm:1006'", []).unwrap();
    let mut previous = store::read_decisions(root).unwrap();
    let item = previous.iter_mut().find(|d| d.key == "gm:1006").unwrap();
    item.rules_version = 2;
    item.outcome = "left_out".into();
    item.reason = Some("calendar_part".into());
    item.left_out_bucket = Some(LeftOutBucket::CalendarResponses);
    item.message_type = None;
    store::write_decisions(root, &previous).unwrap();
    let mut state = store::read_state(root).unwrap();
    state.last_rules_version = 2;
    store::write_state(root, &state).unwrap();

    let prior = client.wire_audit();
    sync::run_header_scan(root, &mut client, now_ts(), &denver_tz(), false).unwrap();
    let corrected = client.wire_audit();
    assert_eq!(corrected.body_section_requests, 0);
    assert_eq!(corrected.header_requests, prior.header_requests + 1);
    let h = crate::cache::HeaderCache::open(&store::header_cache_path(root))
        .unwrap()
        .get("gm:1006")
        .unwrap()
        .unwrap();
    assert_eq!(h.tree.children[2].filename.as_deref(), Some("café.pdf"));
    assert!(h.attachment_names_checked);
    assert_eq!(
        store::read_decisions(root)
            .unwrap()
            .iter()
            .find(|d| d.key == "gm:1006")
            .unwrap()
            .outcome,
        "selected"
    );
    sync::run_header_scan(root, &mut client, now_ts(), &denver_tz(), false).unwrap();
    assert_eq!(
        client.wire_audit().header_requests,
        corrected.header_requests
    );
    client.logout().unwrap();
    let log = handle.join().unwrap();
    assert!(
        log.iter()
            .any(|line| line.contains("UID FETCH 6 (") && line.contains("BODYSTRUCTURE"))
    );
}

#[test]
fn attachment_choice_enriches_existing_exchanges_and_retries_one_occurrence() {
    use crate::attachment_prepare::{PrepareFailure, PreparedAttachment};
    use crate::choice::AttachmentScope;
    use crate::error::Result;
    use crate::imap::{MailboxInfo, WireAudit};
    use crate::model::{Addr, Header, Location, Part};

    struct Mail {
        headers: Vec<Header>,
        downloads: Vec<(String, Vec<u32>)>,
        damaged: bool,
    }
    impl ImapSource for Mail {
        fn mailbox_info(&mut self) -> Result<MailboxInfo> {
            Ok(MailboxInfo {
                name: "All Mail".into(),
                uid_validity: 1,
                uid_next: self.headers.len() as u32 + 1,
                exists: self.headers.len() as u32,
            })
        }
        fn scan_page(&mut self, _: &MailboxInfo, lo: u32, hi: u32) -> Result<Vec<Header>> {
            Ok(self
                .headers
                .iter()
                .filter(|h| (lo..=hi).contains(&h.location.uid))
                .cloned()
                .collect())
        }
        fn fetch_texts(&mut self, reqs: &[(&Header, &Part)]) -> Result<Vec<Option<String>>> {
            Ok(reqs.iter().map(|_|Some(r#"<p>Here it is <img src="cid:sketch"></p><div class="gmail_signature"><img src="cid:logo"></div>"#.into())).collect())
        }
        fn fetch_attachment(
            &mut self,
            h: &Header,
            p: &Part,
            encoded: usize,
            decoded: usize,
        ) -> Result<Option<Vec<u8>>> {
            assert!(p.size <= encoded as u64);
            assert!(decoded <= crate::attachment_parts::HARD_LIMIT);
            self.downloads.push((h.key.clone(), p.path.clone()));
            if self.damaged && p.filename.as_deref() == Some("retry.pdf") {
                return Err(crate::error::EmailError::Unreachable(
                    "temporary read failure".into(),
                ));
            }
            Ok(Some(
                format!("contents for {} {:?}", h.key, p.path).into_bytes(),
            ))
        }
        fn wire_audit(&self) -> WireAudit {
            WireAudit::default()
        }
        fn logout(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let base = Timestamp::now();
    let zone = TimeZone::UTC;
    let part = |n, mime: &str, name: Option<&str>, cid: Option<&str>| Part {
        path: vec![n],
        mime: mime.into(),
        size: 42,
        filename: name.map(str::to_owned),
        content_id: cid.map(str::to_owned),
        disposition: name.map(|_| "attachment".into()),
        ..Default::default()
    };
    let message = |uid: u32, owner: bool, parent: Option<&str>, children| {
        let sender = if owner {
            "maya@example.com"
        } else {
            "casey@example.com"
        };
        Header {
            key: format!("gm:{uid}"),
            gm_msgid: Some(uid.to_string()),
            message_id: Some(format!("m{uid}@example.com")),
            from: vec![Addr {
                address: Some(sender.into()),
                name: None,
            }],
            labels: if owner { vec!["\\Sent".into()] } else { vec![] },
            in_reply_to: parent.into_iter().map(str::to_owned).collect(),
            date_ms: Some(base.as_millisecond() - 3 * 86_400_000 - (4 - i64::from(uid)) * 600_000),
            tree: Part {
                mime: "multipart/mixed".into(),
                children,
                ..Default::default()
            },
            attachment_names_checked: true,
            location: Location {
                mailbox: "All Mail".into(),
                uid_validity: 1,
                uid,
            },
            ..Default::default()
        }
    };
    let body = part(1, "text/html", None, None);
    let parent = message(
        1,
        false,
        None,
        vec![
            body.clone(),
            part(2, "application/pdf", Some("parent.pdf"), None),
        ],
    );
    let sent = message(
        2,
        true,
        Some("m1@example.com"),
        vec![
            body.clone(),
            part(2, "application/pdf", Some("match.pdf"), None),
            part(3, "application/pdf", Some("match.pdf"), None),
            part(4, "image/png", None, Some("sketch")),
            part(5, "image/png", None, Some("logo")),
            part(6, "application/pdf", Some("retry.pdf"), None),
            part(7, "text/calendar", Some("invite.ics"), None),
            Part {
                size: crate::attachment_parts::AUTO_ENCODED_LIMIT as u64 + 1,
                ..part(8, "application/pdf", Some("large.pdf"), None)
            },
            Part {
                size: crate::attachment_parts::HARD_LIMIT as u64 + 1,
                ..part(9, "application/pdf", Some("oversize.pdf"), None)
            },
        ],
    );
    let reply = message(3, true, Some("m1@example.com"), vec![body]);
    let mut mail = Mail {
        headers: vec![parent, sent, reply],
        damaged: true,
        downloads: vec![],
    };
    let prepare = |_: &str,
                   mime: &str,
                   bytes: &[u8]|
     -> std::result::Result<PreparedAttachment, PrepareFailure> {
        let image = mime == "image/png";
        Ok(PreparedAttachment {
            kind: if image { "photos" } else { "documents" },
            text: String::from_utf8(bytes.to_vec()).unwrap(),
            partial: false,
            thoughts: usize::from(image),
            preview_jpeg: image.then(|| vec![1, 2, 3]),
        })
    };
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    sync::connect_and_verify(
        root,
        "maya@example.com",
        "abcd1234efgh5678",
        &mut mail,
        base,
    )
    .unwrap();
    sync::run_header_scan(root, &mut mail, base, &zone, false).unwrap();
    let set = |scope, keep_originals| {
        sync::save_choice_with_files(
            root,
            RangePreset::All,
            MessageType::ALL.to_vec(),
            base,
            &zone,
            Some(scope),
            Some(keep_originals),
        )
        .unwrap()
    };
    let run = |mail: &mut Mail| {
        sync::run_sync_with_attachments(
            root,
            mail,
            base,
            &zone,
            Default::default(),
            None,
            Some(&prepare),
        )
        .unwrap()
    };
    set(AttachmentScope::Names, false);
    assert_eq!(run(&mut mail).landed_messages, 2);
    assert!(mail.downloads.is_empty());
    let journal_config = root.join("config/journal.json");
    let set_paused = |paused: bool| {
        let mut config: Value =
            serde_json::from_slice(&std::fs::read(&journal_config).unwrap()).unwrap();
        config["timezone"] = "UTC".into();
        config["processing"]["backfill_paused"] = paused.into();
        std::fs::write(&journal_config, serde_json::to_vec(&config).unwrap()).unwrap();
    };
    set_paused(true);
    set(AttachmentScope::Both, false);
    let preview = crate::attachment_sync::preview(
        root,
        &store::read_choice(root).unwrap().unwrap(),
        &store::read_decisions(root).unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(preview.sent.remaining_images, 2);
    assert_eq!(
        preview.sent.remaining_files, 2,
        "inline images count before the message context is available"
    );
    let mut live = message(
        4,
        true,
        None,
        vec![
            part(1, "text/html", None, None),
            part(2, "image/png", Some("live.png"), None),
        ],
    );
    live.date_ms = Some(base.as_millisecond());
    mail.headers.push(live);
    assert_eq!(run(&mut mail).landed_messages, 1);
    assert_eq!(mail.downloads, vec![("gm:4".into(), vec![2])]);
    assert_eq!(store::read_state(root).unwrap().file_history_done, 0);
    set_paused(false);
    let mut inventory = solstone_core_system::work_inventory::WorkInventory::default();
    inventory.bands[0].eligible = 1;
    inventory.publish(root).unwrap();
    run(&mut mail);
    assert_eq!(
        mail.downloads.len(),
        1,
        "higher-priority live work waits ahead of historical attachments"
    );
    inventory.bands[0].eligible = 0;
    inventory.publish(root).unwrap();
    let limited = sync::run_sync_with_attachments(
        root,
        &mut mail,
        base,
        &zone,
        SyncOptions {
            attachment_pdf_unavailable: true,
            ..Default::default()
        },
        None,
        Some(&prepare),
    )
    .unwrap();
    assert_eq!(limited.landed_messages, 0);
    assert!(
        mail.downloads
            .iter()
            .all(|(k, p)| k == "gm:4" || (k == "gm:2" && p == &[4]))
    );
    let landed = store::read_landed(root).unwrap();
    let rec = landed.iter().find(|r| r.key == "gm:2").unwrap();
    let segment = root.join("chronicle").join(&rec.segment_rel);
    let read_turn = || {
        let lines = std::fs::read_to_string(segment.join("conversation_transcript.jsonl")).unwrap();
        serde_json::from_str::<Value>(lines.lines().nth(1).unwrap()).unwrap()
    };
    let limited_turn = read_turn();
    assert_eq!(
        limited_turn["attachments"][0]["state"],
        "reader_unavailable"
    );
    assert_eq!(limited_turn["attachments"][2]["state"], "ready");
    assert_eq!(run(&mut mail).landed_messages, 0);
    let row = read_turn();
    let list = row["attachments"].as_array().unwrap();
    assert_eq!(list[0]["name"], list[1]["name"]);
    assert_ne!(list[0]["item_ref"], list[1]["item_ref"]);
    for (index, section) in [(0, 2), (1, 3)] {
        let dir = segment
            .join("items")
            .join(list[index]["item_ref"].as_str().unwrap());
        assert!(
            std::fs::read_to_string(dir.join("document_transcript.md"))
                .unwrap()
                .contains(&format!("gm:2 [{section}]"))
        );
        assert!(!dir.join("original.bin").exists());
    }
    assert_eq!(list[2]["state"], "ready");
    assert_eq!(list[3]["state"], "decorative");
    assert_eq!(list[4]["state"], "failed");
    assert_eq!(list[5]["state"], "unsupported");
    assert_eq!(list[6]["state"], "too_large");
    assert_eq!(list[7]["state"], "over_limit");
    assert!(
        crate::attachment_sync::queue_item(
            root,
            &rec.day,
            &rec.segment,
            list[7]["attachment_id"].as_str().unwrap()
        )
        .is_err()
    );
    assert!(!mail.downloads.iter().any(|(_, p)| p == &[8] || p == &[9]));
    assert_eq!(
        row["reply_to"]["attachments"][0]["sender"],
        "casey@example.com"
    );
    assert_eq!(
        mail.downloads.iter().filter(|(k, _)| k == "gm:1").count(),
        1
    );
    assert!(!mail.downloads.iter().any(|(_, p)| p == &[5] || p == &[7]));
    let progress = sync::status_snapshot(root, &zone)
        .unwrap()
        .file_progress
        .unwrap();
    assert_eq!(
        (progress.estimated_thoughts, progress.actual_thoughts),
        (2, 1),
        "the live photograph arrived after the estimate and belongs outside its comparison"
    );
    assert_eq!(progress.exchanges_done, progress.exchanges_total);
    // Power loss after recording a staged item but before publishing the directory and turn.
    let staged_id = list[0]["item_ref"].as_str().unwrap();
    let item_dir = segment.join("items").join(staged_id);
    std::fs::rename(
        &item_dir,
        segment.join("items").join(format!(".{staged_id}.tmp")),
    )
    .unwrap();
    let recovery_db =
        rusqlite::Connection::open(store::config_dir(root).join("attachment-state.sqlite"))
            .unwrap();
    recovery_db
        .execute("UPDATE items SET installing=1 WHERE id=?1", [staged_id])
        .unwrap();
    recovery_db
        .execute("DELETE FROM checked WHERE key=?1", [&rec.key])
        .unwrap();
    drop(recovery_db);
    let before = mail.downloads.len();
    run(&mut mail);
    assert_eq!(mail.downloads.len(), before);
    assert!(item_dir.join("document_transcript.md").is_file());
    set_paused(true);
    crate::attachment_sync::queue_item(
        root,
        &rec.day,
        &rec.segment,
        list[6]["attachment_id"].as_str().unwrap(),
    )
    .unwrap();
    run(&mut mail);
    assert_eq!(mail.downloads.last().unwrap(), &("gm:2".into(), vec![8]));
    assert_eq!(read_turn()["attachments"][6]["state"], "ready");
    assert_eq!(mail.downloads.len(), before + 1);
    set_paused(false);
    set(AttachmentScope::Both, true);
    mail.damaged = false;
    crate::attachment_sync::queue_item(
        root,
        &rec.day,
        &rec.segment,
        list[4]["attachment_id"].as_str().unwrap(),
    )
    .unwrap();
    run(&mut mail);
    assert_eq!(mail.downloads.len(), before + 2);
    assert_eq!(read_turn()["attachments"][4]["state"], "ready");
    let file_dir = segment
        .join("items")
        .join(list[1]["item_ref"].as_str().unwrap());
    std::fs::remove_dir_all(&file_dir).unwrap();
    set(AttachmentScope::Sent, false);
    run(&mut mail);
    assert!(!file_dir.exists());
    assert_eq!(read_turn()["attachments"][1]["state"], "removed");
    assert_eq!(store::read_landed(root).unwrap().len(), 3);
    let retrieved = segment
        .join("items")
        .join(list[4]["attachment_id"].as_str().unwrap());
    mail.headers.clear();
    sync::disconnect(root, base).unwrap();
    assert_eq!(
        std::fs::read(retrieved.join("original.bin")).unwrap(),
        b"contents for gm:2 [6]"
    );
    assert!(retrieved.join("document_transcript.md").is_file());
}
