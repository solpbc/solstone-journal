// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structurally read-only narrow IMAP4rev1 + Gmail client over workspace `rustls`.
//!
//! Every outgoing wire line passes through [`checked_command`], which permits
//! only `CAPABILITY`, `LOGIN`, `LIST`, `EXAMINE`, `UID FETCH` (with `BODY.PEEK`
//! only), `NOOP`, and `LOGOUT`. Nothing that can mark mail read, move, label,
//! append, expunge, delete, or send is expressible or allowed on the wire.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::{Deserialize, Serialize};

use crate::error::{EmailError, Result};
use crate::headers;
use crate::model::{Header, Location, Part};

pub const GMAIL_IMAP_HOST: &str = "imap.gmail.com";
pub const GMAIL_IMAP_PORT: u16 = 993;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_BYTES: usize = 65_536;
const MAX_LITERAL_BYTES: usize = 32 * 1024 * 1024;

/// Content-free wire audit recorded for every IMAP session.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireAudit {
    pub commands: u64,
    pub verbs: BTreeMap<String, u64>,
    pub received_bytes: u64,
    pub read_only_acks: u64,
    pub header_requests: u64,
    pub body_section_requests: u64,
}

impl WireAudit {
    pub fn merge_from(&mut self, other: &Self) {
        self.commands = self.commands.saturating_add(other.commands);
        for (verb, count) in &other.verbs {
            *self.verbs.entry(verb.clone()).or_default() += *count;
        }
        self.received_bytes = self.received_bytes.saturating_add(other.received_bytes);
        self.read_only_acks = self.read_only_acks.saturating_add(other.read_only_acks);
        self.header_requests = self.header_requests.saturating_add(other.header_requests);
        self.body_section_requests = self
            .body_section_requests
            .saturating_add(other.body_section_requests);
    }
}

/// Mailbox metadata returned by `EXAMINE`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailboxInfo {
    pub name: String,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub exists: u32,
}

/// Structurally read-only command allowlist enforced at the transport write seam.
pub fn checked_command(line: &str) -> Result<&'static str> {
    if line.contains(['\r', '\n']) {
        return Err(EmailError::ReadOnlyGuard(
            "command must be a single line".into(),
        ));
    }
    let (_, command) = line
        .split_once(' ')
        .ok_or_else(|| EmailError::ReadOnlyGuard("command has no tag".into()))?;
    let upper = command.to_ascii_uppercase();
    let verb = upper.split_whitespace().next().unwrap_or("");
    let canonical: &'static str = match verb {
        "CAPABILITY" => "CAPABILITY",
        "LOGIN" => "LOGIN",
        "EXAMINE" => "EXAMINE",
        "LIST" => "LIST",
        "LOGOUT" => "LOGOUT",
        "NOOP" => "NOOP",
        "UID" if upper.starts_with("UID FETCH ") => "UID FETCH",
        _ => {
            return Err(EmailError::ReadOnlyGuard(format!(
                "refused non-read-only verb: {verb}"
            )));
        }
    };
    if canonical == "UID FETCH"
        && (upper.contains("BODY[")
            || upper.contains("RFC822 ")
            || upper.contains("RFC822)")
            || upper.contains("RFC822.TEXT")
            || upper.contains("RFC822.HEADER"))
    {
        return Err(EmailError::ReadOnlyGuard(
            "refused non-PEEK body fetch".into(),
        ));
    }
    Ok(canonical)
}

/// Transport wrapper that enforces [`checked_command`] on every outgoing line
/// and counts wire verbs, bytes, and `[READ-ONLY]` acknowledgments.
pub struct AuditedStream<S> {
    inner: S,
    pending: Vec<u8>,
    tail: Vec<u8>,
    audit: Arc<Mutex<WireAudit>>,
}

impl<S> AuditedStream<S> {
    pub fn new(inner: S) -> (Self, Arc<Mutex<WireAudit>>) {
        let audit = Arc::new(Mutex::new(WireAudit::default()));
        (
            Self {
                inner,
                pending: Vec::new(),
                tail: Vec::new(),
                audit: Arc::clone(&audit),
            },
            audit,
        )
    }
}

impl<S: Read> Read for AuditedStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            let mut w = self.audit.lock().unwrap_or_else(|e| e.into_inner());
            w.received_bytes += n as u64;
            self.tail.extend_from_slice(&buf[..n]);
            let acks = self
                .tail
                .windows(11)
                .filter(|win| win.eq_ignore_ascii_case(b"[READ-ONLY]"))
                .count() as u64;
            if acks > 0 {
                w.read_only_acks += acks;
                self.tail.clear();
            } else if self.tail.len() > 64 {
                let keep_from = self.tail.len() - 16;
                self.tail.drain(..keep_from);
            }
        }
        Ok(n)
    }
}

impl<S: Write> Write for AuditedStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        if self.pending.len() > MAX_COMMAND_BYTES {
            self.pending.clear();
            return Err(io::Error::other("IMAP command exceeds safety bound"));
        }
        while let Some(pos) = self.pending.windows(2).position(|w| w == b"\r\n") {
            let frame: Vec<u8> = self.pending.drain(..pos + 2).collect();
            let line = std::str::from_utf8(&frame[..pos]).map_err(io::Error::other)?;
            let verb = checked_command(line).map_err(|e| io::Error::other(e.to_string()))?;
            self.inner.write_all(&frame)?;
            let upper = line.to_ascii_uppercase();
            let mut w = self.audit.lock().unwrap_or_else(|e| e.into_inner());
            w.commands += 1;
            *w.verbs.entry(verb.to_owned()).or_default() += 1;
            if upper.contains("BODY.PEEK[HEADER") {
                w.header_requests += 1;
            }
            if upper.contains("BODY.PEEK[") && !upper.contains("BODY.PEEK[HEADER") {
                w.body_section_requests += 1;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Abstract read-only mail source used by scan and sync engines (backed by
/// [`GmailImap`] in production and an in-memory IMAP server in tests).
pub trait ImapSource {
    fn mailbox_info(&mut self) -> Result<MailboxInfo>;
    fn scan_page(&mut self, mailbox: &MailboxInfo, lo: u32, hi: u32) -> Result<Vec<Header>>;
    fn fetch_texts(&mut self, reqs: &[(&Header, &Part)]) -> Result<Vec<Option<String>>>;
    fn wire_audit(&self) -> WireAudit;
    fn logout(&mut self) -> Result<()>;
}

/// Default maximum consecutive reconnect retries per failed IMAP page/batch.
pub const DEFAULT_RECONNECT_RETRIES: usize = 3;
/// Default base backoff between reconnect attempts on live connections.
pub const DEFAULT_RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

fn is_transient_imap_error(err: &EmailError) -> bool {
    matches!(err, EmailError::Unreachable(_) | EmailError::Protocol(_))
}

/// Reconnecting wrapper around any read-only [`ImapSource`] opener.
///
/// Long scans and large body fetches can encounter idle TCP/TLS resets or
/// transient IMAP drops. On a transient [`EmailError::Unreachable`] or
/// [`EmailError::Protocol`] error, this wrapper opens a fresh read-only IMAP
/// session and retries the exact failed page or batch up to `max_retries`
/// times, preserving the cumulative [`WireAudit`] across sessions. Permanent,
/// authentication, and read-only guard errors are never retried.
pub struct ReconnectingImapSource<F>
where
    F: FnMut() -> Result<Box<dyn ImapSource + Send>>,
{
    inner: Option<Box<dyn ImapSource + Send>>,
    open: F,
    max_retries: usize,
    backoff: Duration,
    prior_audit: WireAudit,
}

impl<F> ReconnectingImapSource<F>
where
    F: FnMut() -> Result<Box<dyn ImapSource + Send>>,
{
    pub fn new(open: F, max_retries: usize, backoff: Duration) -> Self {
        Self {
            inner: None,
            open,
            max_retries,
            backoff,
            prior_audit: WireAudit::default(),
        }
    }

    pub fn with_initial(
        initial: Box<dyn ImapSource + Send>,
        open: F,
        max_retries: usize,
        backoff: Duration,
    ) -> Self {
        Self {
            inner: Some(initial),
            open,
            max_retries,
            backoff,
            prior_audit: WireAudit::default(),
        }
    }

    fn drop_inner(&mut self) {
        if let Some(src) = self.inner.take() {
            self.prior_audit.merge_from(&src.wire_audit());
        }
    }

    fn ensure_connected(&mut self) -> Result<&mut (dyn ImapSource + Send)> {
        if self.inner.is_none() {
            self.inner = Some((self.open)()?);
        }
        Ok(self
            .inner
            .as_deref_mut()
            .expect("connection opened immediately above"))
    }

    fn reconnect(&mut self, attempt: usize, expected_uid_validity: Option<u32>) -> Result<()> {
        self.drop_inner();
        if !self.backoff.is_zero() {
            let factor = u32::try_from(attempt.max(1)).unwrap_or(1);
            std::thread::sleep(self.backoff.saturating_mul(factor));
        }
        let mut fresh = (self.open)()?;
        if let Some(expected) = expected_uid_validity {
            let mb = fresh.mailbox_info()?;
            if mb.uid_validity != expected {
                self.prior_audit.merge_from(&fresh.wire_audit());
                return Err(EmailError::Protocol(format!(
                    "mailbox UIDVALIDITY changed across reconnect ({expected} -> {})",
                    mb.uid_validity
                )));
            }
        }
        self.inner = Some(fresh);
        Ok(())
    }

    fn call_with_reconnect<T, Op>(
        &mut self,
        expected_uid_validity: Option<u32>,
        mut op: Op,
    ) -> Result<T>
    where
        Op: FnMut(&mut (dyn ImapSource + Send)) -> Result<T>,
    {
        let mut retries = 0usize;
        loop {
            let err = match self.ensure_connected() {
                Ok(src) => match op(src) {
                    Ok(value) => return Ok(value),
                    Err(err) => err,
                },
                Err(err) => err,
            };
            if !is_transient_imap_error(&err) || retries >= self.max_retries {
                return Err(err);
            }
            retries += 1;
            let mut reopen_err: Option<EmailError> = None;
            while retries <= self.max_retries {
                match self.reconnect(retries, expected_uid_validity) {
                    Ok(()) => {
                        reopen_err = None;
                        break;
                    }
                    Err(r_err) => {
                        if !is_transient_imap_error(&r_err) || retries >= self.max_retries {
                            return Err(r_err);
                        }
                        reopen_err = Some(r_err);
                        retries += 1;
                    }
                }
            }
            if let Some(final_err) = reopen_err {
                return Err(final_err);
            }
        }
    }
}

impl<F> ImapSource for ReconnectingImapSource<F>
where
    F: FnMut() -> Result<Box<dyn ImapSource + Send>>,
{
    fn mailbox_info(&mut self) -> Result<MailboxInfo> {
        self.call_with_reconnect(None, |src| src.mailbox_info())
    }

    fn scan_page(&mut self, mailbox: &MailboxInfo, lo: u32, hi: u32) -> Result<Vec<Header>> {
        self.call_with_reconnect(Some(mailbox.uid_validity), |src| {
            src.scan_page(mailbox, lo, hi)
        })
    }

    fn fetch_texts(&mut self, reqs: &[(&Header, &Part)]) -> Result<Vec<Option<String>>> {
        let expected_uid_validity = reqs.first().map(|(h, _)| h.location.uid_validity);
        self.call_with_reconnect(expected_uid_validity, |src| src.fetch_texts(reqs))
    }

    fn wire_audit(&self) -> WireAudit {
        let mut merged = self.prior_audit.clone();
        if let Some(src) = &self.inner {
            merged.merge_from(&src.wire_audit());
        }
        merged
    }

    fn logout(&mut self) -> Result<()> {
        if let Some(mut src) = self.inner.take() {
            let res = src.logout();
            self.prior_audit.merge_from(&src.wire_audit());
            res
        } else {
            Ok(())
        }
    }
}

/// Narrow, structurally read-only IMAP4rev1 + Gmail client over any `Read + Write` stream.
pub struct GmailImap<S: Read + Write> {
    reader: BufReader<AuditedStream<S>>,
    audit: Arc<Mutex<WireAudit>>,
    tag_seq: u64,
    all_mail_name: Option<String>,
    examined: Option<MailboxInfo>,
}

pub type TlsImapStream = StreamOwned<ClientConnection, TcpStream>;

impl GmailImap<TlsImapStream> {
    /// Connect over TLS to `imap.gmail.com:993`, authenticate with an app
    /// password, locate `\All`, and `EXAMINE` it read-only.
    pub fn connect_gmail(email: &str, app_password: &str) -> Result<Self> {
        Self::connect_tls(GMAIL_IMAP_HOST, GMAIL_IMAP_PORT, email, app_password)
    }

    /// Connect over TLS to a given IMAP host/port using `rustls` + `webpki-roots`.
    pub fn connect_tls(host: &str, port: u16, email: &str, app_password: &str) -> Result<Self> {
        let mut addrs = (host, port)
            .to_socket_addrs()
            .map_err(|e| EmailError::Unreachable(e.to_string()))?;
        let mut last_err = None;
        let mut connected = None;
        for addr in addrs.by_ref() {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let tcp = connected.ok_or_else(|| {
            EmailError::Unreachable(
                last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| format!("no addresses resolved for {host}:{port}")),
            )
        })?;
        tcp.set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|e| EmailError::Unreachable(e.to_string()))?;
        tcp.set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|e| EmailError::Unreachable(e.to_string()))?;

        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| EmailError::Unreachable(e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| EmailError::Unreachable(e.to_string()))?;
        let conn = ClientConnection::new(Arc::new(config), server_name)
            .map_err(|e| EmailError::Unreachable(e.to_string()))?;
        let tls = StreamOwned::new(conn, tcp);
        Self::from_stream(tls, email, app_password)
    }
}

impl<S: Read + Write> GmailImap<S> {
    /// Authenticate and prepare a read-only session over an already-connected stream.
    pub fn from_stream(stream: S, email: &str, app_password: &str) -> Result<Self> {
        let (audited, audit) = AuditedStream::new(stream);
        let mut client = Self {
            reader: BufReader::new(audited),
            audit,
            tag_seq: 0,
            all_mail_name: None,
            examined: None,
        };
        let greeting = client.read_response_frame()?;
        let greeting_text = greeting.head_str();
        if !greeting_text.to_ascii_uppercase().starts_with("* OK") {
            return Err(classify_server_rejection(&greeting_text));
        }

        let login_cmd = format!(
            "LOGIN {} {}",
            imap_quote(email.trim()),
            imap_quote(app_password)
        );
        let login_resp = client.run_command(&login_cmd)?;
        if !login_resp.ok {
            return Err(classify_login_error(&login_resp.status_line));
        }

        let all_mail = client.find_all_mail()?;
        client.all_mail_name = Some(all_mail);
        client.examine_all_mail()?;
        Ok(client)
    }

    fn next_tag(&mut self) -> String {
        self.tag_seq += 1;
        format!("s{}", self.tag_seq)
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        let inner = self.reader.get_mut();
        inner
            .write_all(line.as_bytes())
            .map_err(map_transport_error)?;
        inner.write_all(b"\r\n").map_err(map_transport_error)?;
        inner.flush().map_err(map_transport_error)?;
        Ok(())
    }

    /// Read one complete IMAP response frame (one CRLF line plus any trailing
    /// `{N}\r\n<N bytes>` literal continuations belonging to that response).
    fn read_response_frame(&mut self) -> Result<ResponseFrame> {
        let mut segments: Vec<Segment> = Vec::new();
        loop {
            let mut line = Vec::new();
            let n = self
                .reader
                .read_until(b'\n', &mut line)
                .map_err(map_transport_error)?;
            if n == 0 {
                return Err(EmailError::Unreachable(
                    "connection closed by server".into(),
                ));
            }
            if line.ends_with(b"\n") {
                line.pop();
            }
            if line.ends_with(b"\r") {
                line.pop();
            }
            let literal_len = parse_trailing_literal_len(&line);
            if let Some((prefix_end, len)) = literal_len {
                if len > MAX_LITERAL_BYTES {
                    return Err(EmailError::Protocol(format!(
                        "IMAP literal {len} exceeds maximum bound"
                    )));
                }
                line.truncate(prefix_end);
                segments.push(Segment::Line(line));
                let mut lit = vec![0u8; len];
                self.reader
                    .read_exact(&mut lit)
                    .map_err(map_transport_error)?;
                segments.push(Segment::Literal(lit));
            } else {
                segments.push(Segment::Line(line));
                break;
            }
        }
        Ok(ResponseFrame { segments })
    }

    fn run_command(&mut self, command: &str) -> Result<CommandResponse> {
        let tag = self.next_tag();
        self.write_line(&format!("{tag} {command}"))?;
        let mut untagged = Vec::new();
        let tag_prefix = format!("{tag} ");
        loop {
            let frame = self.read_response_frame()?;
            let head = frame.head_str();
            if let Some(rest) = head.strip_prefix(&tag_prefix) {
                let upper = rest.to_ascii_uppercase();
                let ok = upper.starts_with("OK");
                return Ok(CommandResponse {
                    ok,
                    status_line: rest.to_owned(),
                    untagged,
                });
            }
            untagged.push(frame);
        }
    }

    fn find_all_mail(&mut self) -> Result<String> {
        let resp = self.run_command("LIST \"\" \"*\"")?;
        if !resp.ok {
            return Err(classify_server_rejection(&resp.status_line));
        }
        for frame in &resp.untagged {
            if let Some((attrs, name)) = parse_list_frame(frame)
                && attrs.iter().any(|a| a.eq_ignore_ascii_case("\\All"))
            {
                return Ok(name);
            }
        }
        Err(EmailError::AllMailHidden)
    }

    fn examine_all_mail(&mut self) -> Result<MailboxInfo> {
        let mailbox = self
            .all_mail_name
            .clone()
            .ok_or(EmailError::AllMailHidden)?;
        let acks_before = self.wire_audit().read_only_acks;
        let resp = self.run_command(&format!("EXAMINE {}", imap_quote(&mailbox)))?;
        if !resp.ok {
            return Err(classify_examine_error(&resp.status_line));
        }
        let acks_after = self.wire_audit().read_only_acks;
        if acks_after <= acks_before
            && !resp
                .status_line
                .to_ascii_uppercase()
                .contains("[READ-ONLY]")
        {
            return Err(EmailError::Protocol(
                "server did not acknowledge [READ-ONLY] on EXAMINE".into(),
            ));
        }
        let mut exists: u32 = 0;
        let mut uid_validity: Option<u32> = None;
        let mut uid_next: Option<u32> = None;
        for frame in &resp.untagged {
            let line = frame.head_str();
            let upper = line.to_ascii_uppercase();
            if let Some(rest) = upper.strip_prefix("* ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() == 2
                    && parts[1] == "EXISTS"
                    && let Ok(n) = parts[0].parse::<u32>()
                {
                    exists = n;
                }
                if let Some(uv) = extract_bracket_u32(&upper, "UIDVALIDITY ") {
                    uid_validity = Some(uv);
                }
                if let Some(un) = extract_bracket_u32(&upper, "UIDNEXT ") {
                    uid_next = Some(un);
                }
            }
        }
        let info = MailboxInfo {
            name: mailbox,
            uid_validity: uid_validity
                .ok_or_else(|| EmailError::Protocol("EXAMINE omitted UIDVALIDITY".into()))?,
            uid_next: uid_next.unwrap_or(1),
            exists,
        };
        self.examined = Some(info.clone());
        Ok(info)
    }
}

impl<S: Read + Write> ImapSource for GmailImap<S> {
    fn mailbox_info(&mut self) -> Result<MailboxInfo> {
        if let Some(info) = &self.examined {
            return Ok(info.clone());
        }
        self.examine_all_mail()
    }

    fn scan_page(&mut self, mb: &MailboxInfo, lo: u32, hi: u32) -> Result<Vec<Header>> {
        if lo == 0 || lo > hi {
            return Ok(Vec::new());
        }
        let fields = headers::SCAN_FIELDS.join(" ");
        let cmd = format!(
            "UID FETCH {lo}:{hi} (UID FLAGS INTERNALDATE RFC822.SIZE X-GM-MSGID X-GM-THRID X-GM-LABELS BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS ({fields})])"
        );
        let resp = self.run_command(&cmd)?;
        if !resp.ok {
            return Err(classify_server_rejection(&resp.status_line));
        }
        let mut out = Vec::new();
        for frame in &resp.untagged {
            let Some(items) = parse_fetch_items(frame) else {
                continue;
            };
            let Some(uid) = find_u32(&items, "UID") else {
                continue;
            };
            if uid < lo || uid > hi {
                continue;
            }
            let mut h = Header {
                location: Location {
                    mailbox: mb.name.clone(),
                    uid_validity: mb.uid_validity,
                    uid,
                },
                ..Header::default()
            };
            if let Some(flags) = find_list(&items, "FLAGS") {
                h.flags = flags.iter().filter_map(ImapVal::as_string_lossy).collect();
            }
            if let Some(idate) = find_str(&items, "INTERNALDATE") {
                h.internal_date_ms = parse_internaldate_ms(&idate);
            }
            if let Some(sz) = find_u64(&items, "RFC822.SIZE") {
                h.size = sz;
            }
            if let Some(mid) = find_str(&items, "X-GM-MSGID") {
                h.gm_msgid = Some(mid);
            }
            if let Some(tid) = find_str(&items, "X-GM-THRID") {
                h.gm_thrid = Some(tid);
            }
            if let Some(labels) = find_list(&items, "X-GM-LABELS") {
                h.labels = labels.iter().filter_map(ImapVal::as_string_lossy).collect();
            }
            if let Some(bs) = find_value(&items, "BODYSTRUCTURE") {
                h.tree = parse_bodystructure(bs, Vec::new());
            }
            if let Some(raw_hdr) = find_body_header_bytes(&items) {
                headers::fill_from_raw_headers(&mut h, raw_hdr);
            }
            h.key = h
                .gm_msgid
                .as_deref()
                .map(|m| format!("gm:{m}"))
                .unwrap_or_else(|| format!("{}-{}-{}", mb.name, mb.uid_validity, uid));
            out.push(h);
        }
        out.sort_by_key(|h| h.location.uid);
        Ok(out)
    }

    fn fetch_texts(&mut self, reqs: &[(&Header, &Part)]) -> Result<Vec<Option<String>>> {
        let mut groups: BTreeMap<Vec<u32>, Vec<usize>> = BTreeMap::new();
        for (idx, (_, part)) in reqs.iter().enumerate() {
            let path = if part.path.is_empty() {
                vec![1]
            } else {
                part.path.clone()
            };
            groups.entry(path).or_default().push(idx);
        }
        let mut out: Vec<Option<String>> = vec![None; reqs.len()];
        for (path, idxs) in groups {
            let part_str = path
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".");
            let mime_key = format!("BODY[{part_str}.MIME]");
            let body_key = format!("BODY[{part_str}]");
            for chunk in idxs.chunks(50) {
                let uid_set = chunk
                    .iter()
                    .map(|&i| reqs[i].0.location.uid.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let cmd = format!(
                    "UID FETCH {uid_set} (UID BODY.PEEK[{part_str}.MIME] BODY.PEEK[{part_str}])"
                );
                let resp = self.run_command(&cmd)?;
                if !resp.ok {
                    return Err(classify_server_rejection(&resp.status_line));
                }
                type FetchedPartPair = (Option<Vec<u8>>, Option<Vec<u8>>);
                let mut by_uid: HashMap<u32, FetchedPartPair> = HashMap::new();
                for frame in &resp.untagged {
                    let Some(items) = parse_fetch_items(frame) else {
                        continue;
                    };
                    let Some(uid) = find_u32(&items, "UID") else {
                        continue;
                    };
                    let mime_bytes = find_bytes(&items, &mime_key).map(ToOwned::to_owned);
                    let body_bytes = find_bytes(&items, &body_key).map(ToOwned::to_owned);
                    let entry = by_uid.entry(uid).or_insert((None, None));
                    if mime_bytes.is_some() {
                        entry.0 = mime_bytes;
                    }
                    if body_bytes.is_some() {
                        entry.1 = body_bytes;
                    }
                }
                for &i in chunk {
                    let uid = reqs[i].0.location.uid;
                    if let Some((mime, Some(body))) = by_uid.get(&uid) {
                        out[i] = headers::decode_fetched_part(reqs[i].1, mime.as_deref(), body);
                    }
                }
            }
        }
        Ok(out)
    }

    fn wire_audit(&self) -> WireAudit {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn logout(&mut self) -> Result<()> {
        let _ = self.run_command("LOGOUT");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire encoding & error classification
// ---------------------------------------------------------------------------

fn imap_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        if ch != '\r' && ch != '\n' {
            out.push(ch);
        }
    }
    out.push('"');
    out
}

fn map_transport_error(err: io::Error) -> EmailError {
    let msg = err.to_string();
    if msg.contains("read-only IMAP guard") {
        EmailError::ReadOnlyGuard(msg)
    } else {
        EmailError::Unreachable(msg)
    }
}

fn classify_login_error(status: &str) -> EmailError {
    let lower = status.to_ascii_lowercase();
    if lower.contains("imap access is disabled")
        || lower.contains("not enabled for imap")
        || lower.contains("imap is disabled")
    {
        EmailError::ImapDisabled
    } else if lower.contains("authenticationfailed")
        || lower.contains("invalid credentials")
        || lower.contains("application-specific password")
        || lower.contains("bad")
        || lower.contains("no")
    {
        EmailError::AuthFailed
    } else {
        EmailError::Protocol(status.to_owned())
    }
}

fn classify_examine_error(status: &str) -> EmailError {
    let lower = status.to_ascii_lowercase();
    if lower.contains("imap access is disabled") || lower.contains("not enabled for imap") {
        EmailError::ImapDisabled
    } else if lower.contains("nonexistent") || lower.contains("unknown mailbox") {
        EmailError::AllMailHidden
    } else {
        EmailError::Protocol(status.to_owned())
    }
}

fn classify_server_rejection(status: &str) -> EmailError {
    let lower = status.to_ascii_lowercase();
    if lower.contains("imap access is disabled") || lower.contains("not enabled for imap") {
        EmailError::ImapDisabled
    } else if lower.contains("authenticationfailed") || lower.contains("invalid credentials") {
        EmailError::AuthFailed
    } else {
        EmailError::Protocol(status.to_owned())
    }
}

fn extract_bracket_u32(upper_line: &str, key: &str) -> Option<u32> {
    let pos = upper_line.find(key)? + key.len();
    let num: String = upper_line[pos..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    num.parse().ok()
}

// ---------------------------------------------------------------------------
// Response frame & recursive-descent IMAP S-expression parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Segment {
    Line(Vec<u8>),
    Literal(Vec<u8>),
}

#[derive(Debug)]
struct ResponseFrame {
    segments: Vec<Segment>,
}

impl ResponseFrame {
    fn head_str(&self) -> String {
        match self.segments.first() {
            Some(Segment::Line(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
            _ => String::new(),
        }
    }
}

struct CommandResponse {
    ok: bool,
    status_line: String,
    untagged: Vec<ResponseFrame>,
}

fn parse_trailing_literal_len(line: &[u8]) -> Option<(usize, usize)> {
    if !line.ends_with(b"}") {
        return None;
    }
    let open = line.iter().rposition(|&b| b == b'{')?;
    let inner = std::str::from_utf8(&line[open + 1..line.len() - 1]).ok()?;
    let inner = inner.strip_suffix('+').unwrap_or(inner);
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let len = inner.parse::<usize>().ok()?;
    Some((open, len))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ImapVal {
    Atom(String),
    Bytes(Vec<u8>),
    Nil,
    List(Vec<ImapVal>),
}

impl ImapVal {
    fn as_string_lossy(&self) -> Option<String> {
        match self {
            Self::Atom(s) => Some(s.clone()),
            Self::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            Self::Nil | Self::List(_) => None,
        }
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b.as_slice()),
            Self::Atom(s) => Some(s.as_bytes()),
            Self::Nil | Self::List(_) => None,
        }
    }

    fn as_list(&self) -> Option<&[ImapVal]> {
        match self {
            Self::List(l) => Some(l.as_slice()),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum Tok {
    LParen,
    RParen,
    Atom(String),
    Bytes(Vec<u8>),
}

fn tokenize_frame(frame: &ResponseFrame) -> Vec<Tok> {
    let mut toks = Vec::new();
    for seg in &frame.segments {
        match seg {
            Segment::Literal(bytes) => toks.push(Tok::Bytes(bytes.clone())),
            Segment::Line(bytes) => tokenize_line_bytes(bytes, &mut toks),
        }
    }
    toks
}

fn tokenize_line_bytes(bytes: &[u8], out: &mut Vec<Tok>) {
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        let b = bytes[i];
        if b == b' ' || b == b'\t' {
            i += 1;
            continue;
        }
        if b == b'(' {
            out.push(Tok::LParen);
            i += 1;
            continue;
        }
        if b == b')' {
            out.push(Tok::RParen);
            i += 1;
            continue;
        }
        if b == b'"' {
            i += 1;
            let mut val = Vec::new();
            while i < n {
                if bytes[i] == b'\\' && i + 1 < n {
                    val.push(bytes[i + 1]);
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    val.push(bytes[i]);
                    i += 1;
                }
            }
            out.push(Tok::Bytes(val));
            continue;
        }
        // Atom — special-case `BODY[...]` (which can contain spaces and `(...)`,
        // e.g. `BODY[HEADER.FIELDS (FROM TO)]`) so it stays a single key atom.
        let start = i;
        let mut bracket_depth = 0usize;
        while i < n {
            let c = bytes[i];
            if c == b'[' {
                bracket_depth += 1;
                i += 1;
            } else if c == b']' && bracket_depth > 0 {
                bracket_depth -= 1;
                i += 1;
            } else if bracket_depth == 0 && (c == b' ' || c == b'\t' || c == b'(' || c == b')') {
                break;
            } else {
                i += 1;
            }
        }
        let atom = String::from_utf8_lossy(&bytes[start..i]).into_owned();
        if !atom.is_empty() {
            out.push(Tok::Atom(atom));
        }
    }
}

fn parse_values(toks: &[Tok], pos: &mut usize) -> Vec<ImapVal> {
    let mut vals = Vec::new();
    while *pos < toks.len() {
        match &toks[*pos] {
            Tok::RParen => break,
            _ => {
                if let Some(v) = parse_one(toks, pos) {
                    vals.push(v);
                } else {
                    break;
                }
            }
        }
    }
    vals
}

fn parse_one(toks: &[Tok], pos: &mut usize) -> Option<ImapVal> {
    let tok = toks.get(*pos)?;
    *pos += 1;
    match tok {
        Tok::LParen => {
            let inner = parse_values(toks, pos);
            if matches!(toks.get(*pos), Some(Tok::RParen)) {
                *pos += 1;
            }
            Some(ImapVal::List(inner))
        }
        Tok::RParen => None,
        Tok::Bytes(b) => Some(ImapVal::Bytes(b.clone())),
        Tok::Atom(a) => {
            if a.eq_ignore_ascii_case("NIL") {
                Some(ImapVal::Nil)
            } else {
                Some(ImapVal::Atom(a.clone()))
            }
        }
    }
}

fn parse_list_frame(frame: &ResponseFrame) -> Option<(Vec<String>, String)> {
    let toks = tokenize_frame(frame);
    let mut pos = 0;
    let vals = parse_values(&toks, &mut pos);
    // Expected: * LIST (<attrs>) "<delim>" <mailbox-name>
    if vals.len() < 5 {
        return None;
    }
    if vals[0].as_string_lossy().as_deref() != Some("*") {
        return None;
    }
    if !vals[1]
        .as_string_lossy()
        .is_some_and(|s| s.eq_ignore_ascii_case("LIST"))
    {
        return None;
    }
    let attrs: Vec<String> = vals[2]
        .as_list()?
        .iter()
        .filter_map(ImapVal::as_string_lossy)
        .collect();
    let name = vals[4].as_string_lossy()?;
    Some((attrs, name))
}

fn parse_fetch_items(frame: &ResponseFrame) -> Option<Vec<(String, ImapVal)>> {
    let toks = tokenize_frame(frame);
    let mut pos = 0;
    let vals = parse_values(&toks, &mut pos);
    // Expected: * <seq> FETCH (<key> <val> ...)
    if vals.len() < 4 {
        return None;
    }
    if vals[0].as_string_lossy().as_deref() != Some("*") {
        return None;
    }
    if !vals[2]
        .as_string_lossy()
        .is_some_and(|s| s.eq_ignore_ascii_case("FETCH"))
    {
        return None;
    }
    let list = vals[3].as_list()?;
    let mut pairs = Vec::with_capacity(list.len() / 2);
    let mut idx = 0;
    while idx + 1 < list.len() {
        if let Some(key) = list[idx].as_string_lossy() {
            pairs.push((key.to_ascii_uppercase(), list[idx + 1].clone()));
        }
        idx += 2;
    }
    Some(pairs)
}

fn find_value<'a>(items: &'a [(String, ImapVal)], key: &str) -> Option<&'a ImapVal> {
    items
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

fn find_str(items: &[(String, ImapVal)], key: &str) -> Option<String> {
    find_value(items, key).and_then(ImapVal::as_string_lossy)
}

fn find_u32(items: &[(String, ImapVal)], key: &str) -> Option<u32> {
    find_str(items, key)?.parse().ok()
}

fn find_u64(items: &[(String, ImapVal)], key: &str) -> Option<u64> {
    find_str(items, key)?.parse().ok()
}

fn find_list<'a>(items: &'a [(String, ImapVal)], key: &str) -> Option<&'a [ImapVal]> {
    find_value(items, key).and_then(ImapVal::as_list)
}

fn find_bytes<'a>(items: &'a [(String, ImapVal)], key: &str) -> Option<&'a [u8]> {
    find_value(items, key).and_then(ImapVal::as_bytes)
}

fn find_body_header_bytes(items: &[(String, ImapVal)]) -> Option<&[u8]> {
    items
        .iter()
        .find(|(k, _)| k.starts_with("BODY[HEADER"))
        .and_then(|(_, v)| v.as_bytes())
}

// ---------------------------------------------------------------------------
// BODYSTRUCTURE -> Part tree parser
// ---------------------------------------------------------------------------

fn param_map(val: Option<&ImapVal>) -> Vec<(String, String)> {
    let Some(list) = val.and_then(ImapVal::as_list) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < list.len() {
        if let (Some(k), Some(v)) = (list[i].as_string_lossy(), list[i + 1].as_string_lossy()) {
            out.push((k.to_ascii_lowercase(), v));
        }
        i += 2;
    }
    out
}

fn lookup_param(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn parse_disposition(val: Option<&ImapVal>) -> (Option<String>, Option<String>) {
    let Some(list) = val.and_then(ImapVal::as_list) else {
        return (None, None);
    };
    let disp = list
        .first()
        .and_then(ImapVal::as_string_lossy)
        .map(|s| s.to_ascii_lowercase());
    let params = param_map(list.get(1));
    let filename = lookup_param(&params, "filename");
    (disp, filename)
}

fn parse_bodystructure(val: &ImapVal, path: Vec<u32>) -> Part {
    let Some(list) = val.as_list() else {
        return Part::default();
    };
    if list.is_empty() {
        return Part::default();
    }
    // Multipart: starts with one or more sub-part lists, followed by subtype string.
    if list[0].as_list().is_some() {
        let mut child_vals = Vec::new();
        let mut idx = 0;
        while idx < list.len() && list[idx].as_list().is_some() {
            child_vals.push(&list[idx]);
            idx += 1;
        }
        let subtype = list
            .get(idx)
            .and_then(ImapVal::as_string_lossy)
            .unwrap_or_else(|| "mixed".into())
            .to_ascii_lowercase();
        let params = param_map(list.get(idx + 1));
        let (disposition, disp_filename) = parse_disposition(list.get(idx + 2));
        let mut out = Part {
            path: path.clone(),
            mime: format!("multipart/{subtype}"),
            start: lookup_param(&params, "start"),
            disposition,
            filename: disp_filename.or_else(|| lookup_param(&params, "name")),
            ..Part::default()
        };
        for (i, child) in child_vals.into_iter().enumerate() {
            let mut p = path.clone();
            p.push(i as u32 + 1);
            out.children.push(parse_bodystructure(child, p));
        }
        return out;
    }

    // Single-part body:
    // [0] type, [1] subtype, [2] params, [3] content-id, [4] desc, [5] encoding, [6] size
    // Optional specific fields:
    // - text/*: [7] lines, [8] md5, [9] disposition
    // - message/rfc822: [7] envelope, [8] body, [9] lines, [10] md5, [11] disposition
    // - other: [7] md5, [8] disposition
    let ctype = list
        .first()
        .and_then(ImapVal::as_string_lossy)
        .unwrap_or_else(|| "text".into())
        .to_ascii_lowercase();
    let subtype = list
        .get(1)
        .and_then(ImapVal::as_string_lossy)
        .unwrap_or_else(|| "plain".into())
        .to_ascii_lowercase();
    let params = param_map(list.get(2));
    let content_id = list.get(3).and_then(ImapVal::as_string_lossy);
    let encoding = list
        .get(5)
        .and_then(ImapVal::as_string_lossy)
        .map(|s| s.to_ascii_lowercase());
    let size = list
        .get(6)
        .and_then(ImapVal::as_string_lossy)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let disp_idx = if ctype == "text" {
        9
    } else if ctype == "message" && subtype == "rfc822" {
        11
    } else {
        8
    };
    let (disposition, disp_filename) = parse_disposition(list.get(disp_idx));
    Part {
        path: if path.is_empty() { vec![1] } else { path },
        mime: format!("{ctype}/{subtype}"),
        charset: lookup_param(&params, "charset"),
        format: lookup_param(&params, "format"),
        delsp: lookup_param(&params, "delsp"),
        method: lookup_param(&params, "method"),
        start: None,
        content_id,
        encoding,
        disposition,
        filename: disp_filename.or_else(|| lookup_param(&params, "name")),
        size,
        children: Vec::new(),
    }
}

/// Parse an RFC 3501 `INTERNALDATE` (`"18-Nov-2026 10:16:00 -0700"`) into Unix ms.
#[must_use]
pub fn parse_internaldate_ms(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if let Ok(ts) = Timestamp::strptime("%d-%b-%Y %H:%M:%S %z", trimmed) {
        return Some(ts.as_millisecond());
    }
    if let Ok(ts) = Timestamp::strptime("%e-%b-%Y %H:%M:%S %z", trimmed) {
        return Some(ts.as_millisecond());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_guard_refuses_mutating_and_non_peek_commands() {
        for bad in [
            "s1 SELECT INBOX",
            "s1 UID STORE 1 +FLAGS (\\Seen)",
            "s1 STORE 1 +FLAGS (\\Deleted)",
            "s1 CLOSE",
            "s1 EXPUNGE",
            "s1 APPEND INBOX {5}",
            "s1 DELETE Trash",
            "s1 COPY 1:2 Archive",
            "s1 MOVE 1:2 Archive",
            "s1 UID SEARCH ALL",
            "s1 UID FETCH 1 BODY[]",
            "s1 UID FETCH 1 (BODY[1])",
            "s1 UID FETCH 1 (RFC822)",
            "s1 UID FETCH 1 (RFC822.TEXT)",
            "s1 NOOP\r\ns2 EXPUNGE",
        ] {
            assert!(checked_command(bad).is_err(), "expected refusal for: {bad}");
        }
        assert_eq!(
            checked_command(
                "s1 UID FETCH 1:250 (UID FLAGS BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM TO)])"
            )
            .unwrap(),
            "UID FETCH"
        );
        assert_eq!(checked_command("s2 LIST \"\" \"*\"").unwrap(), "LIST");
        assert_eq!(
            checked_command("s3 EXAMINE \"[Gmail]/All Mail\"").unwrap(),
            "EXAMINE"
        );
    }

    #[test]
    fn parses_bodystructure_multipart_with_inline_and_attachment() {
        let line = b"* 1 FETCH (UID 42 BODYSTRUCTURE (((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\" \"FORMAT\" \"flowed\") NIL NIL \"QUOTED-PRINTABLE\" 120 4 NIL NIL NIL)(\"TEXT\" \"HTML\" (\"CHARSET\" \"UTF-8\") NIL NIL \"QUOTED-PRINTABLE\" 340 8 NIL NIL NIL) \"ALTERNATIVE\" (\"BOUNDARY\" \"alt\") NIL NIL)(\"APPLICATION\" \"PDF\" (\"NAME\" \"agenda.pdf\") NIL NIL \"BASE64\" 8192 NIL (\"ATTACHMENT\" (\"FILENAME\" \"agenda.pdf\")) NIL)(\"IMAGE\" \"PNG\" (\"NAME\" \"chart.png\") \"<cid1>\" NIL \"BASE64\" 2048 NIL (\"INLINE\" (\"FILENAME\" \"chart.png\")) NIL) \"MIXED\" (\"BOUNDARY\" \"mix\") NIL NIL))";
        let frame = ResponseFrame {
            segments: vec![Segment::Line(line.to_vec())],
        };
        let items = parse_fetch_items(&frame).expect("fetch items");
        assert_eq!(find_u32(&items, "UID"), Some(42));
        let tree = parse_bodystructure(find_value(&items, "BODYSTRUCTURE").unwrap(), Vec::new());
        assert_eq!(tree.mime, "multipart/mixed");
        assert_eq!(tree.children.len(), 3);
        let choice = crate::model::choose_body(&tree);
        assert_eq!(choice.plain.as_ref().unwrap().path, vec![1, 1]);
        assert_eq!(
            choice.plain.as_ref().unwrap().format.as_deref(),
            Some("flowed")
        );
        assert_eq!(choice.html.as_ref().unwrap().path, vec![1, 2]);
        assert_eq!(choice.skipped_attachments, 2);
        let h = Header {
            tree,
            ..Header::default()
        };
        assert_eq!(h.attachment_names(), vec!["agenda.pdf"]);
        assert_eq!(h.embedded_names(), vec!["chart.png"]);
    }

    #[test]
    fn parses_internaldate_rfc3501() {
        let ms = parse_internaldate_ms("18-Nov-2026 10:16:00 -0700").expect("timestamp");
        let ts = Timestamp::from_millisecond(ms).unwrap();
        assert_eq!(ts.to_string(), "2026-11-18T17:16:00Z");
    }
}
