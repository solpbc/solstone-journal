// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal's own loopback routes, the same ones its browser pages and the
//! Mac app call: the journal's name, its identity and mark, setup state and actions,
//! devices and pairing, whether the journal is open to the
//! network, and the running version. Loopback callers need no credential; nothing here
//! leaves this PC.

use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use crate::setup_flow::{ActionErrorPayload, SetupAction, action_body};

pub const DEFAULT_PORT: u16 = 5015;

/// How long a quick read may take before the window treats the journal as not
/// answering.
const ANSWER_BOUND: Duration = Duration::from_secs(5);

/// Setup's reads and actions run the journal's checks of this PC. On a small
/// PC that is still starting the journal's services for the first time they
/// can take several seconds, and a slow answer is not a stopped setup.
const SETUP_ANSWER_BOUND: Duration = Duration::from_secs(30);

/// The bound for one route: setup's own routes may answer slowly.
fn answer_bound(path: &str) -> Duration {
    if path.starts_with("/init/") {
        SETUP_ANSWER_BOUND
    } else {
        ANSWER_BOUND
    }
}

pub struct Convey {
    base: String,
    agent: Agent,
}

/// A journal answer that wasn't a success. The page reads `reason_code` to
/// offer the way forward the journal's own pages offer.
#[derive(Debug, Eq, PartialEq)]
pub struct Refusal {
    pub status: Option<u16>,
    pub reason_code: Option<String>,
    pub message: String,
}

impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self {
            status: None,
            reason_code: None,
            message,
        }
    }
}

/// What `GET /init` says about the first run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitProbe {
    Complete,
    Incomplete,
}

impl Convey {
    pub fn new(port: Option<u16>) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(ANSWER_BOUND))
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build()
            .into();
        Self {
            base: format!("http://127.0.0.1:{}", port.unwrap_or(DEFAULT_PORT)),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn json(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value, Refusal> {
        let mut response = response.map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| error.to_string())?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(value);
        }
        Err(refusal(status, &value))
    }

    pub fn get(&self, path: &str) -> Result<Value, Refusal> {
        self.json(
            self.agent
                .get(format!("{}{path}", self.base))
                .config()
                .timeout_global(Some(answer_bound(path)))
                .build()
                .header("Accept", "application/json")
                .call(),
        )
    }

    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<Value, Refusal> {
        let request = self
            .agent
            .post(format!("{}{path}", self.base))
            .config()
            .timeout_global(Some(answer_bound(path)))
            .build()
            .header("Accept", "application/json");
        self.json(match body {
            Some(body) => request
                .header("Content-Type", "application/json")
                .send(body.to_string()),
            None => request.send_empty(),
        })
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value, Refusal> {
        self.json(
            self.agent
                .put(format!("{}{path}", self.base))
                .config()
                .timeout_global(Some(answer_bound(path)))
                .build()
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send(body.to_string()),
        )
    }

    /// The journal's setup state, as the setup window reads it.
    pub fn setup_state(&self) -> Result<Value, String> {
        self.get("/init/api/state")
            .map_err(|refusal| refusal.message)
    }

    /// Apply one setup action; the journal answers with its setup state.
    pub fn setup_action(&self, action: &SetupAction) -> Result<Value, ActionErrorPayload> {
        self.post("/init/api/action", Some(&action_body(action)))
            .map_err(|refusal| ActionErrorPayload {
                reason_code: refusal.reason_code,
                message: refusal.message,
                is_transport: refusal.status.is_none(),
            })
    }

    /// Whether a pairing link has been used, by the nonce pair-start returned.
    pub fn pairing_status(&self, nonce: &str) -> Result<Value, Refusal> {
        if !is_nonce(nonce) {
            return Err(Refusal::from("that isn't a pairing link".to_owned()));
        }
        self.get(&format!("/app/network/api/pair/nonce-status?nonce={nonce}"))
    }

    /// Whether the journal answers, and its version when it says. A journal
    /// still in its first run answers every page with that first run, so an
    /// answer of any kind counts; only a finished journal states its version.
    pub fn answer(&self) -> (bool, Option<String>) {
        let Ok(mut response) = self
            .agent
            .get(format!("{}/api/system/status", self.base))
            .header("Accept", "application/json")
            .call()
        else {
            return (false, None);
        };
        let version = response
            .body_mut()
            .read_to_string()
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|status| {
                status
                    .pointer("/version/current")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        (true, version)
    }
    /// `/init` redirects once the first run is finished, as the Mac app reads it.
    pub fn init_probe(&self) -> Result<InitProbe, String> {
        let response = self
            .agent
            .get(format!("{}/init", self.base))
            .header("Accept", "text/html")
            .call()
            .map_err(|error| error.to_string())?;
        match response.status().as_u16() {
            300..=399 => Ok(InitProbe::Complete),
            200 => Ok(InitProbe::Incomplete),
            status => Err(format!("your journal answered {status}")),
        }
    }
}

/// The journal names a refusal for the owner in `detail` or `error`, and its
/// kind in `reason_code`.
fn refusal(status: u16, value: &Value) -> Refusal {
    let message = ["detail", "error", "message"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map_or_else(|| format!("your journal answered {status}"), str::to_owned);
    Refusal {
        status: Some(status),
        reason_code: value
            .get("reason_code")
            .and_then(Value::as_str)
            .map(str::to_owned),
        message,
    }
}

/// A pair-start nonce goes into a query string as it is, so it may only be
/// the characters the journal mints.
fn is_nonce(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce.len() <= 128
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{is_nonce, refusal};

    #[test]
    fn a_refusal_keeps_its_reason_code_for_the_page() {
        let closed = refusal(
            409,
            &json!({"reason_code": "local_network_closed", "detail": "closed"}),
        );
        assert_eq!(closed.status, Some(409));
        assert_eq!(closed.reason_code.as_deref(), Some("local_network_closed"));
        assert_eq!(closed.message, "closed");
        assert_eq!(
            refusal(502, &json!(null)).message,
            "your journal answered 502"
        );
    }

    #[test]
    fn only_a_minted_nonce_reaches_the_query_string() {
        assert!(is_nonce("0123abcdEF-_"));
        assert!(!is_nonce(""));
        assert!(!is_nonce("a&nonce=b"));
        assert!(!is_nonce("a b"));
        assert!(!is_nonce(&"a".repeat(129)));
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_tests {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        use serde_json::{Value, json};

        use super::super::Convey;
        use crate::setup_flow::{Effect, Event, Phase, Session, SetupAction, Step, reduce, view};

        /// Longer than a quick read is allowed, as the journal answered setup
        /// on a two-core PC while its services were first starting.
        const SLOW: Duration = Duration::from_secs(6);

        fn download() -> Value {
            json!({"state": "idle", "attempt_id": null, "bytes_done": 0,
                   "bytes_total": null, "error": null})
        }

        /// A first-run journal that answers its setup routes, slowly for the
        /// reads that run its checks of this PC.
        struct SlowJournal {
            mark: &'static str,
            choice: Option<String>,
            done: bool,
            first_read: bool,
        }

        impl SlowJournal {
            fn state(&self) -> Value {
                let proposed = self.mark != "unavailable";
                let mut remaining = Vec::new();
                if self.mark != "locked" {
                    remaining.push("mark");
                }
                if self.choice.is_none() {
                    remaining.push("thinking");
                }
                if self.done {
                    remaining.clear();
                }
                json!({
                    "schema": "solstone.setup-state.v1",
                    "journal_ready": true,
                    "mark": {
                        "state": self.mark,
                        "proposal_id": if proposed { json!("first") } else { Value::Null },
                        "render": if proposed { json!({
                            "icon1": {"svg": "<path d=\"M0 0h10v10H0z\"/>", "rot": 0,
                                      "color": {"name": "blue", "hex": "#0000ff"}},
                            "icon2": {"svg": "<circle cx=\"5\" cy=\"5\" r=\"5\"/>", "rot": 45,
                                      "color": {"name": "red", "hex": "#ff0000"}},
                            "words": ["north", "stone"]
                        }) } else { Value::Null },
                    },
                    "thinking": {
                        "choice": self.choice,
                        "saved": self.choice.is_some(),
                        "capability": "unknown",
                        "download_bytes": null
                    },
                    "downloads": {"speech": download(), "thinking": download()},
                    "remaining_required_steps": remaining,
                    "done": self.done,
                    "redirect": "/app/home",
                    "copy": {}
                })
            }

            /// Apply one request and say whether its answer is a slow one.
            fn answer(&mut self, method: &str, path: &str, body: &Value) -> (Value, bool) {
                match (method, path, body["action"].as_str()) {
                    ("GET", "/init/api/state", _) => {
                        let slow = self.first_read;
                        self.first_read = false;
                        (self.state(), slow)
                    }
                    ("POST", "/init/api/action", Some("regenerate_mark")) => {
                        self.mark = "proposed";
                        (self.state(), false)
                    }
                    ("POST", "/init/api/action", Some("commit_mark")) => {
                        self.mark = "locked";
                        (self.state(), false)
                    }
                    ("POST", "/init/api/action", Some("save_thinking")) => {
                        self.choice = body["choice"].as_str().map(str::to_owned);
                        (self.state(), true)
                    }
                    ("POST", "/init/api/action", Some("finalize")) => {
                        self.done = true;
                        (self.state(), true)
                    }
                    other => panic!("the window asked an unexpected route: {other:?}"),
                }
            }
        }

        fn serve(journal: Arc<Mutex<SlowJournal>>, mut stream: TcpStream) {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("request line");
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let path = parts.next().unwrap_or_default().to_owned();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("header");
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().expect("content length");
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).expect("body");
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let (answer, slow) = journal.lock().unwrap().answer(&method, &path, &body);
            if slow {
                std::thread::sleep(SLOW);
            }
            let text = answer.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }

        /// Run the window's journal effects the way the app does, until none
        /// is left. Each reply is applied before the next effect runs.
        fn settle(mut session: Session, mut effects: Vec<Effect>, port: u16) -> Session {
            while !effects.is_empty() {
                let effect = effects.remove(0);
                let generation = session.inflight.unwrap_or(0);
                let event = match effect {
                    Effect::FetchState => Event::StateLoaded {
                        generation,
                        result: Convey::new(Some(port)).setup_state(),
                    },
                    Effect::PostAction(action) => Event::ActionCompleted {
                        generation,
                        result: Convey::new(Some(port)).setup_action(&action),
                    },
                    Effect::RecordPending {
                        generation,
                        timezone,
                    } => Event::PendingRecorded {
                        generation,
                        timezone,
                        result: Ok(()),
                    },
                    _ => continue,
                };
                let step: Step = reduce(&session, &event);
                session = step.session;
                assert!(
                    view(&session)["error"].is_null(),
                    "the owner saw an error: {}",
                    view(&session)
                );
                effects.extend(step.effects);
            }
            session
        }

        #[test]
        fn a_slow_setup_answer_still_reaches_home() {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("address").port();
            let journal = Arc::new(Mutex::new(SlowJournal {
                mark: "unavailable",
                choice: None,
                done: false,
                first_read: true,
            }));
            let served = journal.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let journal = served.clone();
                    let stream = stream.expect("accept");
                    std::thread::spawn(move || serve(journal, stream));
                }
            });

            let started = Instant::now();
            let mut session = Session::default();
            for event in [
                Event::Boot {
                    task_installed: false,
                },
                Event::CopyLoaded {
                    generation: 1,
                    result: Ok(json!({})),
                },
                Event::AcknowledgeWelcome,
                Event::ContinueSignIn,
                Event::ChooseFolder(PathBuf::from(r"C:\Users\owner\journal")),
                Event::StartSetup,
            ] {
                session = reduce(&session, &event).session;
            }
            let finished = reduce(
                &session,
                &Event::SetupFinished {
                    generation: session.inflight.expect("setup is running"),
                    result: Ok(None),
                },
            );
            session = settle(finished.session, finished.effects, port);
            assert_eq!(session.phase, Phase::Mark, "{}", view(&session));

            let commit = reduce(
                &session,
                &Event::CommitMarkClicked {
                    proposal_id: "first".into(),
                },
            );
            assert_eq!(
                commit.effects,
                vec![Effect::PostAction(SetupAction::CommitMark {
                    proposal_id: "first".into()
                })]
            );
            session = settle(commit.session, commit.effects, port);
            assert_eq!(session.phase, Phase::Thinking, "{}", view(&session));

            let continued = reduce(
                &session,
                &Event::ContinueThinking {
                    choice: "later".into(),
                    timezone: None,
                },
            );
            session = settle(continued.session, continued.effects, port);
            assert_eq!(session.phase, Phase::Home, "{}", view(&session));

            // The first read, the save and the finish each answered slowly.
            assert!(started.elapsed() >= SLOW * 3, "{:?}", started.elapsed());
            assert!(journal.lock().unwrap().done);
        }
    }
}
