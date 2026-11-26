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
            assert_eq!(row["reply_to"]["author"], "Casey");
            assert!(
                row["reply_to"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Thursday afternoon")
            );
            assert_eq!(row["attachments"][0]["name"], "agenda.pdf");
            assert_eq!(row["attachments"][1]["name"], "site-photo.jpg");
            assert_eq!(row["attachments"][1]["inline"], true);
        }
        if rec.key == "gm:1005" {
            saw_references_only = true;
            assert_eq!(row["email"]["parent_match_rule"], "references_only");
            assert_eq!(row["reply_to"]["author"], "Casey");
        }
        if rec.key == "gm:1004" {
            saw_forwarded = true;
            assert!(row["text"].as_str().unwrap().contains("Looping you in"));
            assert_eq!(row["forwarded"]["subject"], "Room schedule");
            assert!(
                row["forwarded"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Room seven is free")
            );
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

    // Simulate an older release (rules_version=1) having left out UID 10 (gm:1010)
    // and never having landed it, while gm:1002 is already landed at version 1.
    let mut decisions = store::read_decisions(root).unwrap();
    for d in &mut decisions {
        if d.key == "gm:1010" {
            d.outcome = "left_out".into();
            d.message_type = None;
            d.reason = Some("legacy_skip".into());
            d.left_out_bucket = Some(LeftOutBucket::AutomaticReplies);
            d.rules_version = 1;
        }
    }
    store::write_decisions(root, &decisions).unwrap();
    let mut landed = store::read_landed(root).unwrap();
    let gm1002_before = landed.iter().find(|r| r.key == "gm:1002").unwrap().clone();
    landed.retain(|r| r.key != "gm:1010");
    store::write_landed(root, &landed).unwrap();

    // At stale_before=1 (no upgrade), gm:1010 remains skipped.
    let (stream, handle) = mock_server(msgs.clone()).spawn();
    let mut client =
        GmailImap::from_stream(stream, "maya@example.com", "abcd1234efgh5678").unwrap();
    let no_upgrade = sync::run_sync(root, &mut client, now, &tz, SyncOptions::default()).unwrap();
    assert_eq!(no_upgrade.landed_messages, 0);
    client.logout().unwrap();
    let _ = handle.join();

    // Release bumps rules_version=2, stale_before_version=2 -> gm:1010 re-evaluated and lands,
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
            rules_version: Some(2),
            stale_before_version: Some(2),
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
