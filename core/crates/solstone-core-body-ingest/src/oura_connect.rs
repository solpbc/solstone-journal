// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use getrandom::fill as fill_random;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};

use crate::approval::pin_journal_target;
use crate::bundle::{BodyIngestError, BodyIngestErrorKind};
use crate::oura_sync::{hold_oura_lock, post_oura_token_form};

const CALLBACK_ADDR: &str = "127.0.0.1:8765";
const CALLBACK_URL: &str = "http://localhost:8765/callback";
const AUTH_URL: &str = "https://cloud.ouraring.com/oauth/authorize";
const MAX_REQUEST_BYTES: usize = 8192;
const OAUTH_SCOPES: [&str; 9] = [
    "daily",
    "heartrate",
    "workout",
    "tag",
    "session",
    "spo2",
    "stress",
    "heart_health",
    "metabolic",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OuraConnectOptions {
    pub timeout_seconds: u64,
}

impl Default for OuraConnectOptions {
    fn default() -> Self {
        Self {
            timeout_seconds: 300,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OuraConnectReport {
    scopes: Vec<String>,
}

impl OuraConnectReport {
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
}

struct OuraClient {
    client_id: String,
    client_secret: Option<String>,
}

struct TokenValues {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_at: f64,
}

trait ConnectPlatform {
    fn authorization_code(
        &mut self,
        authorization_url: &str,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, BodyIngestError>;

    fn exchange(&mut self, form: &BTreeMap<String, String>) -> Result<Value, BodyIngestError>;
}

struct LiveConnectPlatform;

impl ConnectPlatform for LiveConnectPlatform {
    fn authorization_code(
        &mut self,
        authorization_url: &str,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, BodyIngestError> {
        let listener = TcpListener::bind(CALLBACK_ADDR)
            .map_err(|_| source("authorization_callback_unavailable"))?;
        listener
            .set_nonblocking(true)
            .map_err(|_| source("authorization_callback"))?;
        open_browser(authorization_url)?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| source("authorization_timeout"))?;
        loop {
            if Instant::now() >= deadline {
                return Err(source("authorization_timeout"));
            }
            match listener.accept() {
                Ok((mut stream, _)) => match read_callback(&mut stream, expected_state) {
                    Ok(Some(code)) => return Ok(code),
                    Ok(None) => {}
                    Err(_) => {
                        let _ = callback_response(&mut stream, 400, "Oura authorization rejected.");
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return Err(source("authorization_callback")),
            }
        }
    }

    fn exchange(&mut self, form: &BTreeMap<String, String>) -> Result<Value, BodyIngestError> {
        post_oura_token_form(form)
    }
}

pub fn connect_oura(
    journal: &Path,
    options: &OuraConnectOptions,
) -> Result<OuraConnectReport, BodyIngestError> {
    let mut platform = LiveConnectPlatform;
    connect_with_platform(journal, options, &mut platform)
}

fn connect_with_platform(
    journal: &Path,
    options: &OuraConnectOptions,
    platform: &mut dyn ConnectPlatform,
) -> Result<OuraConnectReport, BodyIngestError> {
    let journal = pin_journal_target(journal)?;
    let journal = journal.as_path();
    if options.timeout_seconds == 0 {
        return Err(source("authorization_timeout"));
    }
    let client = read_client(journal)?;
    let state = random_token(32)?;
    let verifier = random_token(64)?;
    let challenge = base64_url_no_pad(&Sha256::digest(verifier.as_bytes()));
    let scopes = OAUTH_SCOPES.map(str::to_owned).to_vec();
    let authorization_url = authorization_url(&client.client_id, &state, &challenge, &scopes);
    let code = platform.authorization_code(
        &authorization_url,
        &state,
        Duration::from_secs(options.timeout_seconds),
    )?;
    if code.is_empty() {
        return Err(source("authorization_callback"));
    }
    let _lock = hold_oura_lock(journal)?;
    let mut form = BTreeMap::from([
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("code".to_owned(), code),
        ("redirect_uri".to_owned(), CALLBACK_URL.to_owned()),
        ("client_id".to_owned(), client.client_id),
        ("code_verifier".to_owned(), verifier),
    ]);
    if let Some(secret) = client.client_secret {
        form.insert("client_secret".to_owned(), secret);
    }
    let payload = platform.exchange(&form)?;
    let tokens = token_values(&payload)?;
    persist_tokens(journal, tokens)?;
    Ok(OuraConnectReport { scopes })
}

fn read_client(journal: &Path) -> Result<OuraClient, BodyIngestError> {
    let read = read_journal_config(journal).map_err(|_| source("journal_config"))?;
    let config = read
        .config
        .ok_or_else(|| source("journal_config_missing"))?;
    let section = config
        .get("oura")
        .and_then(Value::as_object)
        .ok_or_else(|| source("oura_config_missing"))?;
    let client_id = section
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| source("client_id_missing"))?;
    let client_secret = section
        .get("client_secret")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(OuraClient {
        client_id,
        client_secret,
    })
}

fn token_values(payload: &Value) -> Result<TokenValues, BodyIngestError> {
    let object = payload
        .as_object()
        .ok_or_else(|| source("authorization_response"))?;
    let string = |name| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| source("authorization_response"))
    };
    let expires_at = object
        .get("expires_at")
        .and_then(Value::as_f64)
        .or_else(|| {
            object
                .get("expires_in")
                .and_then(Value::as_f64)
                .map(|seconds| chrono::Utc::now().timestamp() as f64 + seconds)
        })
        .filter(|value| value.is_finite())
        .ok_or_else(|| source("authorization_response"))?;
    Ok(TokenValues {
        access_token: string("access_token")?,
        refresh_token: string("refresh_token")?,
        token_type: object
            .get("token_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("Bearer")
            .to_owned(),
        expires_at,
    })
}

fn persist_tokens(journal: &Path, tokens: TokenValues) -> Result<(), BodyIngestError> {
    mutate_journal_config(journal, move |config| {
        let section = config
            .entry("oura".to_owned())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("the client configuration was checked as an object");
        let next = json!({
            "access_token": tokens.access_token,
            "refresh_token": tokens.refresh_token,
            "expires_at": tokens.expires_at,
            "token_type": tokens.token_type,
        });
        let changed = section.get("tokens") != Some(&next);
        section.insert("tokens".to_owned(), next);
        JournalConfigMutation { changed, value: () }
    })
    .map(|_| ())
    .map_err(|_| publication("token_store"))
}

fn authorization_url(client_id: &str, state: &str, challenge: &str, scopes: &[String]) -> String {
    let values = BTreeMap::from([
        ("client_id", client_id.to_owned()),
        ("code_challenge", challenge.to_owned()),
        ("code_challenge_method", "S256".to_owned()),
        ("redirect_uri", CALLBACK_URL.to_owned()),
        ("response_type", "code".to_owned()),
        ("scope", scopes.join(" ")),
        ("state", state.to_owned()),
    ]);
    let query = values
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{AUTH_URL}?{query}")
}

fn read_callback(
    stream: &mut TcpStream,
    expected_state: &str,
) -> Result<Option<String>, BodyIngestError> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| source("authorization_callback"))?;
    let mut bytes = Vec::new();
    let mut limited = stream.take((MAX_REQUEST_BYTES + 1) as u64);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = limited
            .read(&mut chunk)
            .map_err(|_| source("authorization_callback"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(source("authorization_callback"));
    }
    let request = std::str::from_utf8(&bytes).map_err(|_| source("authorization_callback"))?;
    let line = request
        .split("\r\n")
        .next()
        .ok_or_else(|| source("authorization_callback"))?;
    let mut parts = line.split(' ');
    if parts.next() != Some("GET") {
        return Err(source("authorization_callback"));
    }
    let target = parts
        .next()
        .ok_or_else(|| source("authorization_callback"))?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(source("authorization_callback"));
    }
    let code = callback_code(target, expected_state)?;
    match code {
        Some(code) => {
            callback_response(
                stream,
                200,
                "Oura authorization received. You can return to Solstone.",
            )?;
            Ok(Some(code))
        }
        None => {
            callback_response(stream, 400, "Oura authorization rejected.")?;
            Ok(None)
        }
    }
}

fn callback_code(target: &str, expected_state: &str) -> Result<Option<String>, BodyIngestError> {
    let (path, query) = target
        .split_once('?')
        .ok_or_else(|| source("authorization_callback"))?;
    if path != "/callback" {
        return Ok(None);
    }
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        values
            .entry(percent_decode(key)?)
            .or_default()
            .push(percent_decode(value)?);
    }
    let one = |name| match values.get(name).map(Vec::as_slice) {
        Some([value]) if !value.is_empty() => Some(value.as_str()),
        _ => None,
    };
    if one("state") != Some(expected_state) {
        return Ok(None);
    }
    Ok(one("code").map(str::to_owned))
}

fn callback_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
) -> Result<(), BodyIngestError> {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|_| source("authorization_callback"))
}

fn random_token(bytes: usize) -> Result<String, BodyIngestError> {
    let mut raw = vec![0_u8; bytes];
    fill_random(&mut raw).map_err(|_| source("authorization_random"))?;
    Ok(base64_url_no_pad(&raw))
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[(first >> 2) as usize]));
        output.push(char::from(
            ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize],
            ));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[(third & 0x3f) as usize]));
        }
    }
    output
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> Result<String, BodyIngestError> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                decoded.push((hex(bytes[index + 1])? << 4) | hex(bytes[index + 2])?);
                index += 3;
            }
            b'%' => return Err(source("authorization_callback")),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| source("authorization_callback"))
}

fn hex(byte: u8) -> Result<u8, BodyIngestError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(source("authorization_callback")),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn open_browser(url: &str) -> Result<(), BodyIngestError> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    spawn_browser_command(&mut command)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn spawn_browser_command(command: &mut Command) -> Result<(), BodyIngestError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| source("authorization_browser"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_browser(_url: &str) -> Result<(), BodyIngestError> {
    Err(source("authorization_browser"))
}

const fn source(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Source, stage)
}

const fn publication(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Publication, stage)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = env::temp_dir().join(format!("solstone-oura-connect-{stamp}"));
            fs::create_dir_all(path.join("config")).expect("config directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakePlatform {
        expected_url: Option<String>,
        expected_state: Option<String>,
        exchange: Value,
        form: Option<BTreeMap<String, String>>,
    }

    impl ConnectPlatform for FakePlatform {
        fn authorization_code(
            &mut self,
            authorization_url: &str,
            expected_state: &str,
            _timeout: Duration,
        ) -> Result<String, BodyIngestError> {
            self.expected_url = Some(authorization_url.to_owned());
            self.expected_state = Some(expected_state.to_owned());
            Ok("synthetic-code".to_owned())
        }

        fn exchange(&mut self, form: &BTreeMap<String, String>) -> Result<Value, BodyIngestError> {
            self.form = Some(form.clone());
            Ok(self.exchange.clone())
        }
    }

    #[test]
    fn pkce_connect_uses_bound_state_and_persists_only_tokens() {
        let journal = TempDir::new();
        fs::write(
            journal.0.join("config/journal.json"),
            serde_json::to_vec(&json!({
                "identity": {"timezone": "UTC"},
                "oura": {
                    "client_id": "synthetic-client",
                    "client_secret": "synthetic-secret",
                    "preserved": {"key": true}
                }
            }))
            .expect("config"),
        )
        .expect("write config");
        let mut platform = FakePlatform {
            expected_url: None,
            expected_state: None,
            exchange: json!({
                "access_token": "synthetic-access",
                "refresh_token": "synthetic-refresh",
                "token_type": "Bearer",
                "expires_at": 4102444800.0
            }),
            form: None,
        };
        let report =
            connect_with_platform(&journal.0, &OuraConnectOptions::default(), &mut platform)
                .expect("connect");
        assert_eq!(report.scopes(), OAUTH_SCOPES);
        let url = platform.expected_url.expect("authorization URL");
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A8765%2Fcallback"));
        let state = platform.expected_state.expect("state");
        assert!(url.contains(&format!("state={state}")));
        let form = platform.form.expect("token form");
        assert_eq!(form["code"], "synthetic-code");
        assert_eq!(form["client_secret"], "synthetic-secret");
        assert!(form["code_verifier"].len() >= 43);

        let config: Value = serde_json::from_slice(
            &fs::read(journal.0.join("config/journal.json")).expect("saved config"),
        )
        .expect("saved config JSON");
        assert_eq!(config["oura"]["preserved"]["key"], true);
        assert_eq!(config["oura"]["tokens"]["access_token"], "synthetic-access");
        assert_eq!(
            config["oura"]["tokens"]["refresh_token"],
            "synthetic-refresh"
        );
        assert!(!url.contains("synthetic-secret"));
    }

    #[test]
    fn callback_parser_rejects_state_confusion_duplicates_and_invalid_encoding() {
        assert_eq!(
            callback_code("/callback?code=hello%2Bworld&state=expected", "expected")
                .expect("valid callback"),
            Some("hello+world".to_owned())
        );
        assert_eq!(
            callback_code("/callback?code=one&state=wrong", "expected").expect("wrong state"),
            None
        );
        assert_eq!(
            callback_code("/callback?code=one&code=two&state=expected", "expected")
                .expect("duplicate code"),
            None
        );
        assert!(callback_code("/callback?code=%GG&state=expected", "expected").is_err());
        assert_eq!(
            callback_code("/other?code=one&state=expected", "expected").unwrap(),
            None
        );
    }

    #[test]
    fn base64_url_encoding_pins_pkce_boundaries() {
        assert_eq!(base64_url_no_pad(b""), "");
        assert_eq!(base64_url_no_pad(b"f"), "Zg");
        assert_eq!(base64_url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64_url_no_pad(b"foo"), "Zm9v");
        assert_eq!(
            base64_url_no_pad(&Sha256::digest(b"synthetic-verifier")),
            "SwisrY-odM5E0NhbqyXlh9EaF96vyb-VtU1Zb4xw37I"
        );
    }

    #[cfg(unix)]
    #[test]
    fn browser_opener_cannot_hold_or_contaminate_the_helper_protocol() {
        let started = Instant::now();
        let output = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--ignored",
                "--exact",
                "oura_connect::tests::browser_stdio_child",
                "--nocapture",
            ])
            .env("SOLSTONE_BROWSER_STDIO_CHILD", "1")
            .output()
            .expect("run isolated opener process");
        assert!(output.status.success());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the outer helper waited for the long-lived browser opener"
        );
        assert!(
            !output
                .stdout
                .windows(18)
                .any(|part| part == b"browser-stdio-leak")
        );
        assert!(
            !output
                .stderr
                .windows(18)
                .any(|part| part == b"browser-stdio-leak")
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "subprocess fixture for browser_opener_cannot_hold_or_contaminate_the_helper_protocol"]
    fn browser_stdio_child() {
        if env::var_os("SOLSTONE_BROWSER_STDIO_CHILD").is_none() {
            return;
        }
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 2; printf browser-stdio-leak"]);
        spawn_browser_command(&mut command).expect("spawn browser fixture");
    }
}
