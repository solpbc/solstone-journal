// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Functional reducer and helpers for the Windows journal window setup flow.
//!
//! Owns setup argv construction, route and origin validation, action serialization,
//! copy string formatting, and the state machine across welcome, sign-in, folder selection,
//! setup progress, mark confirmation, thinking configuration, and landing home.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Windows publication requires setup qualification on the signed candidate.
pub const SETUP_SEQUENCE_ENABLED: bool = true;

pub const APP_HOME_PATH: &str = "/app/home";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Welcome,
    SignIn,
    Folder,
    Opening,
    Setup,
    Mark,
    Thinking,
    Home,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserEffect {
    None,
    OpenHome,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinueOutcome {
    Published,
    Failed,
    Unsupported,
    Superseded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupAction {
    RegenerateMark,
    CommitMark { proposal_id: String },
    SaveThinking { choice: String },
    StartThinkingDownload,
    CancelThinkingDownload { attempt_id: String },
    RetrySpeechDownload,
    Finalize { timezone: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    FetchCopy,
    RunSetup {
        journal: PathBuf,
        task_installed: bool,
        start_at_sign_in: bool,
    },
    FetchState,
    PostAction(SetupAction),
    CheckModels {
        generation: u64,
    },
    InstallModels {
        generation: u64,
    },
    RecordPending {
        generation: u64,
        timezone: Option<String>,
    },
    ContinueInSolstone {
        generation: u64,
    },
    ObserveReceipt {
        generation: u64,
    },
    OpenSolstoneDownload,
    Browser(BrowserEffect),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionErrorPayload {
    pub reason_code: Option<String>,
    pub message: String,
    pub is_transport: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    Boot {
        task_installed: bool,
    },
    AcknowledgeWelcome,
    ChooseSignIn(bool),
    ContinueSignIn,
    ChooseFolder(PathBuf),
    StartSetup,
    RetrySetup,
    SetupFinished {
        generation: u64,
        result: Result<Option<String>, String>,
    },
    CopyLoaded {
        generation: u64,
        result: Result<Value, String>,
    },
    StateLoaded {
        generation: u64,
        result: Result<Value, String>,
    },
    ActionCompleted {
        generation: u64,
        result: Result<Value, ActionErrorPayload>,
    },
    RegenerateMarkClicked,
    CommitMarkClicked {
        proposal_id: String,
    },
    ChooseThinking(String),
    ContinueThinking {
        choice: String,
        timezone: Option<String>,
    },
    StartThinkingDownloadClicked,
    CancelThinkingDownloadClicked {
        attempt_id: String,
    },
    RetrySpeechDownloadClicked,
    FinalizeClicked {
        timezone: Option<String>,
    },
    OpenJournalClicked,
    ModelsCheckFinished {
        generation: u64,
        result: Result<(), String>,
    },
    ModelsInstallFinished {
        generation: u64,
        result: Result<(), String>,
    },
    RetryModelsInstallClicked,
    RequestModelsCheck,
    RetryState,
    Refresh,
    DismissError,
    PendingRecorded {
        generation: u64,
        timezone: Option<String>,
        result: Result<(), String>,
    },
    ContinueInSolstone,
    ContinueFinished {
        generation: u64,
        outcome: ContinueOutcome,
    },
    DownloadSolstoneClicked,
    HomeShown,
    ContinuationObserved {
        generation: u64,
        pending: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub generation: u64,
    pub inflight: Option<u64>,
    pub models_generation: u64,
    pub models_pending: bool,
    pub phase: Phase,
    pub task_installed: bool,
    pub start_at_sign_in: bool,
    pub journal_path: PathBuf,
    pub copy: Option<Value>,
    pub snapshot: Option<Value>,
    pub models_notice: Option<String>,
    pub stored_redirect: Option<String>,
    pub busy: bool,
    pub resting: bool,
    pub setup_failed: bool,
    pub setup_running: bool,
    pub authoritative: bool,
    pub initial_mark_pending: bool,
    pub continue_after_save: Option<(String, Option<String>)>,
    pub error: Option<String>,
    pub continuation_pending: bool,
    pub solstone_download: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            generation: 0,
            inflight: None,
            models_generation: 0,
            models_pending: false,
            phase: Phase::Welcome,
            task_installed: false,
            start_at_sign_in: true,
            journal_path: PathBuf::new(),
            copy: None,
            snapshot: None,
            models_notice: None,
            stored_redirect: None,
            busy: false,
            resting: false,
            setup_failed: false,
            setup_running: false,
            authoritative: false,
            initial_mark_pending: false,
            continue_after_save: None,
            error: None,
            continuation_pending: false,
            solstone_download: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub session: Session,
    pub effects: Vec<Effect>,
}

impl Step {
    pub fn browser_effect(&self) -> BrowserEffect {
        for effect in &self.effects {
            if let Effect::Browser(b) = effect {
                return *b;
            }
        }
        BrowserEffect::None
    }
}

fn parsed_app_url(raw: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(raw).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    if !parsed
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("journal.localhost"))
    {
        return None;
    }
    match parsed.port() {
        None | Some(443) => {}
        Some(_) => return None,
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    Some(parsed)
}

pub fn is_app_origin(raw: &str) -> bool {
    parsed_app_url(raw).is_some()
}

pub fn app_asset_path(raw: &str) -> Option<String> {
    let parsed = parsed_app_url(raw)?;
    let path = parsed.path();
    if path.is_empty() {
        Some("/".to_string())
    } else {
        Some(path.to_string())
    }
}

#[cfg(test)]
pub fn home_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{APP_HOME_PATH}")
}

fn is_remote(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::FetchState
            | Effect::PostAction(_)
            | Effect::ContinueInSolstone { .. }
            | Effect::Browser(BrowserEffect::OpenHome)
    )
}

/// Drop journal calls when no port is bound. `Ok(0)` means nothing here talks
/// to Convey; callers must not open a client for that value.
pub fn withhold_remote(
    session: &mut Session,
    effects: &mut Vec<Effect>,
    port: Option<u16>,
) -> Result<u16, ()> {
    if !effects.iter().any(is_remote) {
        return Ok(0);
    }
    let Some(port) = port else {
        effects.retain(|effect| !is_remote(effect));
        if session.error.is_none() {
            session.error = Some("the journal isn't answering".to_owned());
        }
        session.busy = false;
        session.inflight = None;
        session.authoritative = false;
        session.continue_after_save = None;
        if session.phase == Phase::Opening && session.snapshot.is_none() && session.task_installed {
            session.resting = true;
        }
        return Err(());
    };
    Ok(port)
}

#[derive(Clone, Debug, PartialEq)]
pub enum TurnItem {
    Reply,
    Effect(Effect),
}

pub fn execution_order(effects: &[Effect]) -> Vec<TurnItem> {
    let mut items = Vec::with_capacity(effects.len() + 1);
    items.push(TurnItem::Reply);
    items.extend(effects.iter().cloned().map(TurnItem::Effect));
    items
}

pub fn execute_turn<R, D>(rendered: Value, effects: &[Effect], reply: R, mut dispatch: D)
where
    R: FnOnce(Value),
    D: FnMut(&Effect),
{
    let mut reply = Some(reply);
    for item in execution_order(effects) {
        match item {
            TurnItem::Reply => {
                if let Some(reply) = reply.take() {
                    reply(rendered.clone());
                }
            }
            TurnItem::Effect(effect) => dispatch(&effect),
        }
    }
}

fn arg_string(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

pub fn event_from_intent(intent: &str, args: &Value) -> Result<Event, String> {
    Ok(match intent {
        "boot" => {
            let Some(task_installed) = args.get("task_installed").and_then(Value::as_bool) else {
                return Err("task_installed must be a boolean".to_owned());
            };
            Event::Boot { task_installed }
        }
        "acknowledge" | "acknowledge-welcome" | "acknowledgeWelcome" => Event::AcknowledgeWelcome,
        "choose-sign-in" | "chooseSignIn" => {
            let on = args
                .get("on")
                .or_else(|| args.get("start_at_sign_in"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Event::ChooseSignIn(on)
        }
        "continue-sign-in" | "continueSignIn" => Event::ContinueSignIn,
        "choose-folder" | "chooseFolder" => {
            Event::ChooseFolder(PathBuf::from(arg_string(args, "path")))
        }
        "start-setup" | "startSetup" => Event::StartSetup,
        "retry-setup" | "retrySetup" => Event::RetrySetup,
        "retry-state" | "retryState" => Event::RetryState,
        "regenerate-mark" | "regenerateMark" => Event::RegenerateMarkClicked,
        "commit-mark" | "commitMark" => Event::CommitMarkClicked {
            proposal_id: arg_string(args, "proposal_id"),
        },
        "choose-thinking" | "chooseThinking" => Event::ChooseThinking(arg_string(args, "choice")),
        "continue-thinking" => Event::ContinueThinking {
            choice: arg_string(args, "choice"),
            timezone: args
                .get("timezone")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "start-thinking-download" | "startThinkingDownload" => Event::StartThinkingDownloadClicked,
        "cancel-thinking-download" | "cancelThinkingDownload" => {
            Event::CancelThinkingDownloadClicked {
                attempt_id: arg_string(args, "attempt_id"),
            }
        }
        "retry-speech-download" | "retrySpeechDownload" => Event::RetrySpeechDownloadClicked,
        "finalize" => Event::FinalizeClicked {
            timezone: args
                .get("timezone")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "open-journal" | "openJournal" => Event::OpenJournalClicked,
        "retry-models" | "retryModels" => Event::RetryModelsInstallClicked,
        "refresh" => Event::Refresh,
        "dismiss-error" | "dismissError" => Event::DismissError,
        "continue-solstone" => Event::ContinueInSolstone,
        "download-solstone" => Event::DownloadSolstoneClicked,
        _ => return Err(format!("unknown flow intent {intent}")),
    })
}

pub fn convey_route_allowed(method: &str, path: &str) -> bool {
    if path.contains(['?', '#']) || path.contains("..") {
        return false;
    }
    matches!(
        (method, path),
        ("GET" | "PUT", "/app/settings/api/config")
            | ("GET", "/app/link/api/identity")
            | ("GET", "/app/network/api/devices")
            | ("POST", "/app/network/pair-start")
            | ("POST", "/app/network/unpair")
            | ("GET", "/app/network/api/local-network")
            | ("POST", "/app/network/local-network/open")
            | ("POST", "/app/network/local-network/close")
            | ("GET", "/init/api/state")
            | ("POST", "/init/api/action")
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConveyDispatch {
    Rejected,
    Unanswered,
    Use(Option<u16>),
}

pub fn convey_dispatch(
    method: &str,
    path: &str,
    renderer_port: Option<u16>,
    status_port: Option<u16>,
) -> ConveyDispatch {
    if !convey_route_allowed(method, path) {
        return ConveyDispatch::Rejected;
    }
    match (method, path) {
        ("GET", "/init/api/state") | ("POST", "/init/api/action") => match status_port {
            Some(p) => ConveyDispatch::Use(Some(p)),
            None => ConveyDispatch::Unanswered,
        },
        _ => ConveyDispatch::Use(renderer_port),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelsJob {
    Check,
    Install,
}

pub fn admit_models_job(session: &mut Session, effects: &mut Vec<Effect>, job: ModelsJob) -> bool {
    if session.models_pending {
        return false;
    }
    session.models_generation += 1;
    session.models_pending = true;
    match job {
        ModelsJob::Check => {
            effects.push(Effect::CheckModels {
                generation: session.models_generation,
            });
        }
        ModelsJob::Install => {
            effects.push(Effect::InstallModels {
                generation: session.models_generation,
            });
        }
    }
    true
}

pub fn legacy_setup_argv(journal: &Path) -> Vec<String> {
    vec![
        "setup".into(),
        "--jsonl".into(),
        "--yes".into(),
        "--accept-existing-journal".into(),
        "--journal".into(),
        journal.display().to_string(),
    ]
}

pub fn setup_argv(journal: &Path, task_installed: bool, start_at_sign_in: bool) -> Vec<String> {
    let mut args = vec![
        "setup".to_owned(),
        "--jsonl".to_owned(),
        "--yes".to_owned(),
        "--accept-existing-journal".to_owned(),
        "--skip-models".to_owned(),
        "--skip-brain".to_owned(),
    ];
    if !task_installed {
        args.push("--initial-start-at-sign-in".to_owned());
        args.push(if start_at_sign_in { "on" } else { "off" }.to_owned());
    }
    args.push("--journal".to_owned());
    args.push(journal.display().to_string());
    args
}

fn format_bytes(bytes: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < units.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else if value < 10.0 {
        format!("{:.1} {}", value, units[unit])
    } else {
        format!("{:.0} {}", value, units[unit])
    }
}

pub fn filled_default_line(
    template: &str,
    download_bytes: Option<u64>,
    size_unknown: &str,
) -> String {
    let size_str = match download_bytes {
        Some(bytes) => format_bytes(bytes),
        None => size_unknown.to_string(),
    };
    template.replacen("{size}", &size_str, 1)
}

pub fn action_body(action: &SetupAction) -> Value {
    match action {
        SetupAction::RegenerateMark => json!({
            "action": "regenerate_mark"
        }),
        SetupAction::CommitMark { proposal_id } => json!({
            "action": "commit_mark",
            "proposal_id": proposal_id
        }),
        SetupAction::SaveThinking { choice } => json!({
            "action": "save_thinking",
            "choice": choice
        }),
        SetupAction::StartThinkingDownload => json!({
            "action": "start_thinking_download"
        }),
        SetupAction::CancelThinkingDownload { attempt_id } => json!({
            "action": "cancel_thinking_download",
            "attempt_id": attempt_id
        }),
        SetupAction::RetrySpeechDownload => json!({
            "action": "retry_speech_download"
        }),
        SetupAction::Finalize { timezone } => {
            let mut map = serde_json::Map::new();
            map.insert("action".into(), json!("finalize"));
            if let Some(tz) = timezone.as_ref().filter(|tz| !tz.trim().is_empty()) {
                map.insert("timezone".into(), json!(tz));
            }
            Value::Object(map)
        }
    }
}

pub fn speech_retry(snapshot: Option<&Value>) -> bool {
    let Some(s) = snapshot else { return false };
    let state = s.pointer("/downloads/speech/state").and_then(Value::as_str);
    let error_code = s
        .pointer("/downloads/speech/error/code")
        .and_then(Value::as_str);
    state == Some("error") && error_code != Some("speech_package_unavailable")
}

pub fn thinking_retry(snapshot: Option<&Value>) -> bool {
    let Some(s) = snapshot else { return false };
    let saved = s
        .pointer("/thinking/saved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let choice = s.pointer("/thinking/choice").and_then(Value::as_str);
    let state = s
        .pointer("/downloads/thinking/state")
        .and_then(Value::as_str);
    saved
        && choice == Some("local")
        && matches!(state, Some("error") | Some("cancelled") | Some("idle"))
}

pub fn view(session: &Session) -> Value {
    let mark_render = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/mark/render"))
        .cloned();
    let mark_state = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/mark/state"))
        .and_then(Value::as_str);
    let mark_proposal = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/mark/proposal_id"))
        .and_then(Value::as_str);

    let local_offered = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/thinking/capability"))
        .and_then(Value::as_str)
        == Some("ok");

    let thinking_capability = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/thinking/capability"))
        .and_then(Value::as_str);

    let default_line = match (
        session
            .copy
            .as_ref()
            .and_then(|c| c.pointer("/thinking/default_line"))
            .and_then(Value::as_str),
        session
            .copy
            .as_ref()
            .and_then(|c| c.pointer("/thinking/size_unknown"))
            .and_then(Value::as_str),
    ) {
        (Some(template), Some(size_unknown)) => {
            let download_bytes = session
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/thinking/download_bytes"))
                .and_then(Value::as_u64);
            json!(filled_default_line(template, download_bytes, size_unknown))
        }
        _ => Value::Null,
    };

    let thinking_download_bytes = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/thinking/download_bytes"))
        .and_then(Value::as_u64);

    let downloads = session
        .snapshot
        .as_ref()
        .and_then(|s| s.get("downloads"))
        .cloned()
        .unwrap_or(Value::Null);

    let speech_state = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/downloads/speech/state"))
        .and_then(Value::as_str);
    let speech_retry_val = speech_retry(session.snapshot.as_ref());

    let thinking_saved = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/thinking/saved"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let thinking_choice = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/thinking/choice"))
        .and_then(Value::as_str);
    let thinking_state = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/downloads/thinking/state"))
        .and_then(Value::as_str);
    let thinking_attempt_id = session
        .snapshot
        .as_ref()
        .and_then(|s| s.pointer("/downloads/thinking/attempt_id"))
        .and_then(Value::as_str);

    let thinking_retry_val = thinking_retry(session.snapshot.as_ref());
    let thinking_cancel = thinking_state == Some("downloading") && thinking_attempt_id.is_some();

    let mark_locked = mark_state == Some("locked");
    let remaining_empty = session
        .snapshot
        .as_ref()
        .and_then(|s| s.get("remaining_required_steps"))
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.is_empty());
    let can_finalize = mark_locked && thinking_saved && remaining_empty;

    let watch_downloads =
        speech_state == Some("downloading") || thinking_state == Some("downloading");

    let destination_rejected = session.stored_redirect.as_deref().is_some()
        && session.stored_redirect.as_deref() != Some(APP_HOME_PATH);

    let step: Option<u8> = match session.phase {
        Phase::Welcome | Phase::SignIn => Some(1),
        Phase::Folder => Some(2),
        Phase::Mark => Some(3),
        Phase::Thinking => Some(4),
        Phase::Home | Phase::Opening | Phase::Setup => None,
    };

    json!({
        "phase": session.phase,
        "step": step,
        "busy": session.busy,
        "resting": session.resting,
        "error": session.error,
        "sign_in": session.start_at_sign_in,
        "copy": session.copy,
        "mark": {
            "state": mark_state,
            "proposal_id": mark_proposal,
            "render": mark_render,
        },
        "local_offered": local_offered,
        "default_line": default_line,
        "thinking_download_bytes": thinking_download_bytes,
        "downloads": downloads,
        "speech_retry": speech_retry_val,
        "thinking_retry": thinking_retry_val,
        "thinking_cancel": thinking_cancel,
        "thinking_saved": thinking_saved,
        "thinking_choice": thinking_choice,
        "setup_running": session.phase == Phase::Setup && session.setup_running,
        "setup_failed": session.setup_failed,
        "models": {
            "showing": session.models_notice.is_some(),
            "detail": session.models_notice,
            "pending": session.models_pending,
        },
        "can_finalize": can_finalize,
        "watch_downloads": watch_downloads,
        "destination_rejected": destination_rejected,
        "authoritative": session.authoritative,
        "thinking_capability": thinking_capability,
        "continuation_pending": session.continuation_pending,
        "solstone_download": session.solstone_download,
    })
}

pub fn reduce(session: &Session, event: &Event) -> Step {
    let mut next = session.clone();
    let mut effects = Vec::new();

    match event {
        Event::Boot { task_installed } => {
            next.task_installed = *task_installed;
            next.start_at_sign_in = true;
            next.busy = true;
            next.error = None;
            next.generation += 1;
            next.inflight = Some(next.generation);
            if *task_installed {
                next.phase = Phase::Opening;
                effects.push(Effect::FetchState);
            } else {
                next.phase = Phase::Welcome;
                effects.push(Effect::FetchCopy);
            }
        }
        Event::AcknowledgeWelcome => {
            if next.inflight.is_some() || next.phase != Phase::Welcome {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.phase = Phase::SignIn;
        }
        Event::ChooseSignIn(on) => {
            if next.phase != Phase::SignIn {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.start_at_sign_in = *on;
        }
        Event::ContinueSignIn => {
            if next.phase != Phase::SignIn {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.phase = Phase::Folder;
        }
        Event::ChooseFolder(path) => {
            next.journal_path = path.clone();
        }
        Event::StartSetup => {
            if next.inflight.is_some() || next.phase != Phase::Folder {
                return Step {
                    session: next,
                    effects,
                };
            }
            if next.journal_path.as_os_str().is_empty()
                || next.journal_path.to_string_lossy().trim().is_empty()
            {
                next.error = Some("choose where your journal lives".into());
                return Step {
                    session: next,
                    effects,
                };
            }
            next.setup_failed = false;
            next.setup_running = true;
            next.initial_mark_pending = true;
            next.phase = Phase::Setup;
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::RunSetup {
                journal: next.journal_path.clone(),
                task_installed: next.task_installed,
                start_at_sign_in: next.start_at_sign_in,
            });
        }
        Event::RetrySetup => {
            if next.inflight.is_some() || next.journal_path.as_os_str().is_empty() {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.setup_failed = false;
            next.setup_running = true;
            next.phase = Phase::Setup;
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::RunSetup {
                journal: next.journal_path.clone(),
                task_installed: next.task_installed,
                start_at_sign_in: next.start_at_sign_in,
            });
        }
        Event::SetupFinished { generation, result } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.setup_running = false;
            next.inflight = None;
            next.busy = false;
            match result {
                Ok(models_err) => {
                    next.setup_failed = false;
                    if let Some(text) = models_err {
                        next.models_notice = Some(text.clone());
                    }
                    admit_models_job(&mut next, &mut effects, ModelsJob::Check);
                    next.generation += 1;
                    next.inflight = Some(next.generation);
                    next.busy = true;
                    effects.push(Effect::FetchState);
                }
                Err(err) => {
                    next.phase = Phase::Setup;
                    next.setup_failed = true;
                    next.error = Some(err.clone());
                }
            }
        }
        Event::CopyLoaded { generation, result } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.inflight = None;
            next.busy = false;
            match result {
                Ok(val) => {
                    next.copy = Some(val.clone());
                    next.error = None;
                }
                Err(err) => {
                    next.error = Some(err.clone());
                }
            }
        }
        Event::StateLoaded { generation, result } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.inflight = None;
            next.busy = false;
            match result {
                Ok(state) => {
                    apply_snapshot(&mut next, &mut effects, state);
                    if next.authoritative && next.initial_mark_pending {
                        next.initial_mark_pending = false;
                        if !state.get("done").and_then(Value::as_bool).unwrap_or(false)
                            && state.pointer("/mark/state").and_then(Value::as_str)
                                == Some("unavailable")
                        {
                            let first_mark = reduce(&next, &Event::RegenerateMarkClicked);
                            next = first_mark.session;
                            effects.extend(first_mark.effects);
                        }
                    }
                }
                Err(err) => {
                    next.authoritative = false;
                    if next.error.is_none() {
                        next.error = Some(err.clone());
                    }
                }
            }
        }
        Event::ActionCompleted { generation, result } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.inflight = None;
            next.busy = false;
            let continuation = next.continue_after_save.take();
            match result {
                Ok(state) => {
                    apply_snapshot(&mut next, &mut effects, state);
                    if let Some((choice, timezone)) = continuation
                        && next.authoritative
                        && next.phase == Phase::Thinking
                        && state.pointer("/thinking/saved").and_then(Value::as_bool) == Some(true)
                        && state.pointer("/thinking/choice").and_then(Value::as_str)
                            == Some(choice.as_str())
                    {
                        let finalized = reduce(&next, &Event::FinalizeClicked { timezone });
                        next = finalized.session;
                        effects.extend(finalized.effects);
                    }
                }
                Err(payload) => {
                    next.error = Some(payload.message.clone());
                    next.generation += 1;
                    next.inflight = Some(next.generation);
                    next.busy = true;
                    effects.push(Effect::FetchState);
                }
            }
        }
        Event::RegenerateMarkClicked => {
            if !next.authoritative || next.inflight.is_some() || next.phase != Phase::Mark {
                return Step {
                    session: next,
                    effects,
                };
            }
            let mark_locked = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/mark/state"))
                .and_then(Value::as_str)
                == Some("locked");
            if mark_locked {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::RegenerateMark));
        }
        Event::CommitMarkClicked { proposal_id } => {
            if !next.authoritative || next.inflight.is_some() || next.phase != Phase::Mark {
                return Step {
                    session: next,
                    effects,
                };
            }
            if proposal_id.trim().is_empty() {
                next.error = Some("that mark isn't the one on screen".to_owned());
                return Step {
                    session: next,
                    effects,
                };
            }
            let render_ok = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/mark/render"))
                .is_some_and(|r| admit_mark_render(r).is_ok());
            if !render_ok {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::CommitMark {
                proposal_id: proposal_id.clone(),
            }));
        }
        Event::ChooseThinking(choice) => {
            if !next.authoritative || next.inflight.is_some() || next.phase != Phase::Thinking {
                return Step {
                    session: next,
                    effects,
                };
            }
            if !matches!(choice.as_str(), "local" | "byo" | "spp" | "later") {
                next.error = Some("unknown thinking choice".into());
                return Step {
                    session: next,
                    effects,
                };
            }
            if choice == "local" {
                let capability = next
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.pointer("/thinking/capability"))
                    .and_then(Value::as_str);
                if capability != Some("ok") {
                    next.error = Some("local thinking is unavailable on this device".into());
                    return Step {
                        session: next,
                        effects,
                    };
                }
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::SaveThinking {
                choice: choice.clone(),
            }));
        }
        Event::ContinueThinking { choice, timezone } => {
            let saved_choice_matches = next.snapshot.as_ref().is_some_and(|state| {
                state.pointer("/thinking/saved").and_then(Value::as_bool) == Some(true)
                    && state.pointer("/thinking/choice").and_then(Value::as_str)
                        == Some(choice.as_str())
            });
            if saved_choice_matches {
                return reduce(
                    &next,
                    &Event::FinalizeClicked {
                        timezone: timezone.clone(),
                    },
                );
            }
            let mut saving = reduce(&next, &Event::ChooseThinking(choice.clone()));
            if saving.effects.iter().any(|effect| {
                matches!(effect, Effect::PostAction(SetupAction::SaveThinking { .. }))
            }) {
                saving.session.continue_after_save = Some((choice.clone(), timezone.clone()));
            }
            return saving;
        }
        Event::StartThinkingDownloadClicked => {
            if !next.authoritative
                || next.inflight.is_some()
                || !thinking_retry(next.snapshot.as_ref())
            {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::StartThinkingDownload));
        }
        Event::CancelThinkingDownloadClicked { attempt_id } => {
            if !next.authoritative || next.inflight.is_some() {
                return Step {
                    session: next,
                    effects,
                };
            }
            if attempt_id.trim().is_empty() {
                next.error = Some("that download isn't the one on screen".to_owned());
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::CancelThinkingDownload {
                attempt_id: attempt_id.clone(),
            }));
        }
        Event::RetrySpeechDownloadClicked => {
            if !next.authoritative
                || next.inflight.is_some()
                || !speech_retry(next.snapshot.as_ref())
            {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::PostAction(SetupAction::RetrySpeechDownload));
        }
        Event::FinalizeClicked { timezone } => {
            if !next.authoritative || next.inflight.is_some() || next.phase != Phase::Thinking {
                return Step {
                    session: next,
                    effects,
                };
            }
            let mark_locked = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/mark/state"))
                .and_then(Value::as_str)
                == Some("locked");
            let thinking_saved = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/thinking/saved"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let remaining_empty = next
                .snapshot
                .as_ref()
                .and_then(|s| s.get("remaining_required_steps"))
                .and_then(Value::as_array)
                .is_some_and(|arr| arr.is_empty());

            if !mark_locked || !thinking_saved || !remaining_empty {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::RecordPending {
                generation: next.generation,
                timezone: timezone.clone(),
            });
        }
        Event::OpenJournalClicked => {
            if next.stored_redirect.as_deref() == Some(APP_HOME_PATH) {
                effects.push(Effect::Browser(BrowserEffect::OpenHome));
            } else {
                next.error = Some("unexpected redirect destination".into());
                effects.push(Effect::Browser(BrowserEffect::Rejected));
            }
        }
        Event::ModelsCheckFinished { generation, result } => {
            if *generation != next.models_generation {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.models_pending = false;
            match result {
                Ok(()) => {
                    next.models_notice = None;
                }
                Err(text) => {
                    next.models_notice = Some(text.clone());
                }
            }
        }
        Event::ModelsInstallFinished { generation, result } => {
            if *generation != next.models_generation {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.models_pending = false;
            match result {
                Ok(()) => {
                    admit_models_job(&mut next, &mut effects, ModelsJob::Check);
                }
                Err(text) => {
                    next.models_notice = Some(text.clone());
                }
            }
        }
        Event::RetryModelsInstallClicked => {
            admit_models_job(&mut next, &mut effects, ModelsJob::Install);
        }
        Event::RequestModelsCheck => {
            admit_models_job(&mut next, &mut effects, ModelsJob::Check);
        }
        Event::RetryState => {
            if next.inflight.is_some()
                || (next.phase == Phase::Setup && !next.setup_failed && next.setup_running)
            {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            effects.push(Effect::FetchState);
        }
        Event::Refresh => {
            if next.inflight.is_some() {
                return Step {
                    session: next,
                    effects,
                };
            }
            let speech_downloading = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/downloads/speech/state"))
                .and_then(Value::as_str)
                == Some("downloading");
            let thinking_downloading = next
                .snapshot
                .as_ref()
                .and_then(|s| s.pointer("/downloads/thinking/state"))
                .and_then(Value::as_str)
                == Some("downloading");
            if speech_downloading || thinking_downloading {
                next.generation += 1;
                next.inflight = Some(next.generation);
                next.busy = true;
                effects.push(Effect::FetchState);
            }
        }
        Event::DismissError => {
            next.error = None;
        }
        Event::PendingRecorded {
            generation,
            timezone,
            result,
        } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            match result {
                Ok(()) => {
                    effects.push(Effect::PostAction(SetupAction::Finalize {
                        timezone: timezone.clone(),
                    }));
                }
                Err(_) => {
                    next.inflight = None;
                    next.busy = false;
                    next.error = Some("could not save this step. try again.".to_owned());
                }
            }
        }
        Event::ContinueInSolstone => {
            if next.phase != Phase::Home || !next.authoritative || next.inflight.is_some() {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.generation += 1;
            next.inflight = Some(next.generation);
            next.busy = true;
            next.error = None;
            next.solstone_download = false;
            effects.push(Effect::ContinueInSolstone {
                generation: next.generation,
            });
        }
        Event::ContinueFinished {
            generation,
            outcome,
        } => {
            if Some(*generation) != next.inflight {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.inflight = None;
            next.busy = false;
            match outcome {
                ContinueOutcome::Published => {
                    next.continuation_pending = true;
                }
                ContinueOutcome::Failed => {
                    next.continuation_pending = true;
                    next.error =
                        Some("could not continue in the solstone app. try again.".to_owned());
                }
                ContinueOutcome::Unsupported => {
                    next.continuation_pending = true;
                    next.solstone_download = true;
                }
                ContinueOutcome::Superseded => {}
            }
        }
        Event::DownloadSolstoneClicked => {
            if next.phase == Phase::Home {
                effects.push(Effect::OpenSolstoneDownload);
            }
        }
        Event::HomeShown => {
            if next.phase == Phase::Home && next.inflight.is_none() {
                effects.push(Effect::ObserveReceipt {
                    generation: next.generation,
                });
            }
        }
        Event::ContinuationObserved {
            generation,
            pending,
        } => {
            if *generation != next.generation || next.inflight.is_some() {
                return Step {
                    session: next,
                    effects,
                };
            }
            next.continuation_pending = *pending;
        }
    }

    Step {
        session: next,
        effects,
    }
}

fn string_or_null(value: Option<&Value>) -> Option<Option<&str>> {
    match value {
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.as_str())),
        _ => None,
    }
}

fn nonblank(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

fn path_has_geometry(path: &str) -> bool {
    let mut drawing = false;
    for segment in svgtypes::PathParser::from(path) {
        match segment {
            Ok(svgtypes::PathSegment::MoveTo { .. } | svgtypes::PathSegment::ClosePath { .. }) => {}
            Ok(_) => drawing = true,
            Err(_) => return false,
        }
    }
    drawing
}

fn svg_has_geometry(svg: &str) -> bool {
    use quick_xml::events::Event as XmlEvent;
    let wrapped = format!("<svg>{svg}</svg>");
    let mut reader = quick_xml::Reader::from_str(&wrapped);
    let mut depth = 0_usize;
    let mut geometry = false;
    loop {
        let (element, starts) = match reader.read_event() {
            Ok(XmlEvent::Start(element)) => (element, true),
            Ok(XmlEvent::Empty(element)) => (element, false),
            Ok(XmlEvent::End(_)) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            Ok(XmlEvent::Eof) => return depth == 0 && geometry,
            Err(_) | Ok(XmlEvent::DocType(_) | XmlEvent::Decl(_)) => return false,
            Ok(_) => continue,
        };
        if depth == 1 {
            let mut attrs = std::collections::BTreeMap::new();
            for attribute in element.attributes() {
                let Ok(attribute) = attribute else {
                    return false;
                };
                let Ok(value) = attribute.decoded_and_normalized_value(
                    quick_xml::XmlVersion::Implicit1_0,
                    reader.decoder(),
                ) else {
                    return false;
                };
                attrs.insert(attribute.key.as_ref().to_vec(), value.into_owned());
            }
            let text = |key: &[u8]| attrs.get(key).map(String::as_str).unwrap_or("");
            let number = |key: &[u8]| text(key).parse::<f64>().ok().filter(|n| n.is_finite());
            let positive = |key: &[u8]| number(key).is_some_and(|n| n > 0.0);
            geometry |= match element.name().as_ref() {
                b"path" => path_has_geometry(text(b"d")),
                b"circle" => positive(b"r"),
                b"ellipse" => positive(b"rx") && positive(b"ry"),
                b"rect" => positive(b"width") && positive(b"height"),
                b"line" => {
                    number(b"x1").unwrap_or(0.0) != number(b"x2").unwrap_or(0.0)
                        || number(b"y1").unwrap_or(0.0) != number(b"y2").unwrap_or(0.0)
                }
                b"polyline" | b"polygon" => text(b"points")
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|part| !part.is_empty())
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .is_ok_and(|points| {
                        points.len() >= 4
                            && points.len() % 2 == 0
                            && points.iter().all(|n| n.is_finite())
                    }),
                _ => false,
            };
        }
        if starts {
            depth += 1;
        }
    }
}

pub(crate) fn admit_mark_render(render: &Value) -> Result<(), ()> {
    let obj = render.as_object().ok_or(())?;
    let icon1 = obj.get("icon1").and_then(Value::as_object).ok_or(())?;
    let icon2 = obj.get("icon2").and_then(Value::as_object).ok_or(())?;
    let words = obj.get("words").and_then(Value::as_array).ok_or(())?;
    if words.len() != 2 {
        return Err(());
    }
    for w in words {
        if w.as_str().unwrap_or("").trim().is_empty() {
            return Err(());
        }
    }
    for icon in [icon1, icon2] {
        let svg = icon.get("svg").and_then(Value::as_str).unwrap_or("");
        if !svg_has_geometry(svg) {
            return Err(());
        }
        let rot = icon.get("rot").and_then(Value::as_i64).ok_or(())?;
        if rot != 0 && rot != 45 {
            return Err(());
        }
        let color = icon.get("color").and_then(Value::as_object).ok_or(())?;
        let color_name = color.get("name").and_then(Value::as_str).unwrap_or("");
        if color_name.trim().is_empty() {
            return Err(());
        }
        let hex = color.get("hex").and_then(Value::as_str).unwrap_or("");
        if hex.len() != 7 || !hex.starts_with('#') {
            return Err(());
        }
        if !hex[1..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(());
        }
    }
    Ok(())
}

fn admit_download(value: &Value) -> Result<(), ()> {
    let Some(download) = value.as_object() else {
        return Err(());
    };
    let state = download.get("state").and_then(Value::as_str);
    if !matches!(
        state,
        Some("idle" | "downloading" | "complete" | "cancelled" | "error")
    ) {
        return Err(());
    }
    if string_or_null(download.get("attempt_id")).is_none() {
        return Err(());
    }
    if download.get("bytes_done").and_then(Value::as_u64).is_none() {
        return Err(());
    }
    match download.get("bytes_total") {
        Some(Value::Null) => {}
        Some(val) if val.as_u64().is_some() => {}
        _ => return Err(()),
    }
    match download.get("error") {
        Some(Value::Null) => {
            if state == Some("error") {
                return Err(());
            }
        }
        Some(Value::Object(error)) => {
            if state == Some("error") && error.get("code").and_then(Value::as_str).is_none() {
                return Err(());
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

fn admit_setup_state(state: &Value) -> Result<(), ()> {
    let Some(body) = state.as_object() else {
        return Err(());
    };
    if body.get("schema").and_then(Value::as_str) != Some("solstone.setup-state.v1") {
        return Err(());
    }
    if body.get("journal_ready").and_then(Value::as_bool).is_none() {
        return Err(());
    }
    let Some(mark) = body.get("mark").and_then(Value::as_object) else {
        return Err(());
    };
    let mark_state = mark.get("state").and_then(Value::as_str);
    if !matches!(mark_state, Some("locked" | "proposed" | "unavailable")) {
        return Err(());
    }
    let Some(proposal_id) = string_or_null(mark.get("proposal_id")) else {
        return Err(());
    };
    match mark.get("render") {
        Some(Value::Null) | Some(Value::Object(_)) => {}
        _ => return Err(()),
    }
    let Some(thinking) = body.get("thinking").and_then(Value::as_object) else {
        return Err(());
    };
    let Some(choice) = string_or_null(thinking.get("choice")) else {
        return Err(());
    };
    let Some(saved) = thinking.get("saved").and_then(Value::as_bool) else {
        return Err(());
    };
    if !matches!(
        thinking.get("capability").and_then(Value::as_str),
        Some("ok" | "blocked" | "unknown")
    ) {
        return Err(());
    }
    match thinking.get("download_bytes") {
        Some(Value::Null) => {}
        Some(val) if val.as_u64().is_some() => {}
        _ => return Err(()),
    }
    let Some(downloads) = body.get("downloads").and_then(Value::as_object) else {
        return Err(());
    };
    let Some(speech) = downloads.get("speech") else {
        return Err(());
    };
    let Some(thinking_download) = downloads.get("thinking") else {
        return Err(());
    };
    admit_download(speech)?;
    admit_download(thinking_download)?;
    let Some(remaining) = body
        .get("remaining_required_steps")
        .and_then(Value::as_array)
    else {
        return Err(());
    };
    if !remaining.iter().all(Value::is_string) {
        return Err(());
    }
    let Some(done) = body.get("done").and_then(Value::as_bool) else {
        return Err(());
    };
    let Some(redirect) = body.get("redirect").and_then(Value::as_str) else {
        return Err(());
    };
    if !body.get("copy").is_some_and(Value::is_object) {
        return Err(());
    }
    if done && !remaining.is_empty() {
        return Err(());
    }
    if done && redirect != APP_HOME_PATH {
        return Err(());
    }
    if saved {
        if !matches!(choice, Some("local" | "spp" | "byo" | "later")) {
            return Err(());
        }
    } else if choice.is_some() {
        return Err(());
    }
    if mark_state == Some("proposed") {
        if !nonblank(proposal_id) {
            return Err(());
        }
        let render = mark.get("render").ok_or(())?;
        admit_mark_render(render)?;
    }
    if mark_state == Some("unavailable") && nonblank(proposal_id) {
        return Err(());
    }
    Ok(())
}

fn apply_snapshot(session: &mut Session, effects: &mut Vec<Effect>, state: &Value) {
    if admit_setup_state(state).is_err() {
        session.authoritative = false;
        if session.error.is_none() {
            session.error = Some("the journal's setup state could not be read".to_owned());
        }
        return;
    }
    session.authoritative = true;
    session.resting = false;
    session.snapshot = Some(state.clone());
    if let Some(copy) = state.get("copy").filter(|copy| !copy.is_null()) {
        session.copy = Some(copy.clone());
    }
    session.stored_redirect = state
        .get("redirect")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let done = state.get("done").and_then(Value::as_bool).unwrap_or(false);
    if done {
        session.inflight = None;
        session.busy = false;
        let already_home = session.phase == Phase::Home;
        session.phase = Phase::Home;
        if !already_home {
            admit_models_job(session, effects, ModelsJob::Check);
            effects.push(Effect::ObserveReceipt {
                generation: session.generation,
            });
        }
        return;
    }

    let mark_locked = state.pointer("/mark/state").and_then(Value::as_str) == Some("locked");
    session.inflight = None;
    session.busy = false;
    session.phase = if mark_locked {
        Phase::Thinking
    } else {
        Phase::Mark
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_download() -> Value {
        json!({
            "state": "idle",
            "attempt_id": null,
            "bytes_done": 0,
            "bytes_total": null,
            "error": null
        })
    }

    fn sample_render() -> Value {
        json!({
            "icon1": {
                "svg": "<path d=\"M0 0h10v10H0z\"/>",
                "rot": 0,
                "color": {
                    "name": "blue",
                    "hex": "#0000ff"
                }
            },
            "icon2": {
                "svg": "<circle cx=\"5\" cy=\"5\" r=\"5\"/>",
                "rot": 45,
                "color": {
                    "name": "red",
                    "hex": "#ff0000"
                }
            },
            "words": ["north", "stone"]
        })
    }

    fn setup_state() -> Value {
        json!({
            "schema": "solstone.setup-state.v1",
            "journal_ready": true,
            "mark": {
                "state": "unavailable",
                "proposal_id": null,
                "render": null
            },
            "thinking": {
                "choice": null,
                "saved": false,
                "capability": "ok",
                "download_bytes": null
            },
            "downloads": {
                "speech": idle_download(),
                "thinking": idle_download()
            },
            "remaining_required_steps": ["mark", "thinking"],
            "done": false,
            "redirect": "/app/home",
            "copy": { "heading": "setup" }
        })
    }

    #[test]
    fn unseeded_snapshot_walk_through_regenerate_commit_thinking_and_finalize() {
        let mut session = Session::default();
        let step = reduce(
            &session,
            &Event::Boot {
                task_installed: false,
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Welcome);
        assert_eq!(step.effects, vec![Effect::FetchCopy]);

        let step = reduce(
            &session,
            &Event::CopyLoaded {
                generation: session.inflight.unwrap(),
                result: Ok(json!({ "welcome": { "title": "Welcome" } })),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Welcome);
        assert!(session.inflight.is_none());

        let step = reduce(&session, &Event::AcknowledgeWelcome);
        session = step.session;
        assert_eq!(session.phase, Phase::SignIn);

        let step = reduce(&session, &Event::ChooseSignIn(false));
        session = step.session;
        assert!(!session.start_at_sign_in);
        assert_eq!(session.phase, Phase::SignIn);

        let step = reduce(&session, &Event::ContinueSignIn);
        session = step.session;
        assert_eq!(session.phase, Phase::Folder);

        let step = reduce(
            &session,
            &Event::ChooseFolder(PathBuf::from(r"C:\Users\owner\journal")),
        );
        session = step.session;
        assert_eq!(
            session.journal_path,
            PathBuf::from(r"C:\Users\owner\journal")
        );

        let step = reduce(&session, &Event::StartSetup);
        session = step.session;
        assert_eq!(session.phase, Phase::Setup);
        assert_eq!(
            step.effects,
            vec![Effect::RunSetup {
                journal: PathBuf::from(r"C:\Users\owner\journal"),
                task_installed: false,
                start_at_sign_in: false,
            }]
        );

        let current_gen = session.inflight.unwrap();
        // Stale SetupFinished does not apply
        let stale = reduce(
            &session,
            &Event::SetupFinished {
                generation: current_gen - 1,
                result: Ok(None),
            },
        );
        assert_eq!(stale.session, session);
        assert!(stale.effects.is_empty());

        let step = reduce(
            &session,
            &Event::SetupFinished {
                generation: current_gen,
                result: Ok(None),
            },
        );
        session = step.session;
        assert_eq!(
            step.effects,
            vec![Effect::CheckModels { generation: 1 }, Effect::FetchState,]
        );

        // Folder Continue carries one first-proposal intent through setup.
        let unseeded_state = setup_state();
        let step = reduce(
            &session,
            &Event::StateLoaded {
                generation: session.inflight.unwrap(),
                result: Ok(unseeded_state),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Mark);
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::RegenerateMark)]
        );
        assert!(session.inflight.is_some());
        assert!(!session.initial_mark_pending);
        assert_eq!(view(&session)["busy"], true);

        let mut state_with_prop = setup_state();
        state_with_prop["mark"]["state"] = json!("proposed");
        state_with_prop["mark"]["proposal_id"] = json!("prop-42");
        state_with_prop["mark"]["render"] = sample_render();
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Ok(state_with_prop),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Mark);
        assert!(session.inflight.is_none());
        assert!(step.effects.is_empty());

        // Commit posts the id from the click, even when the snapshot holds another.
        let step = reduce(
            &session,
            &Event::CommitMarkClicked {
                proposal_id: "shown-id".into(),
            },
        );
        session = step.session;
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::CommitMark {
                proposal_id: "shown-id".into(),
            })]
        );

        let mut state_mark_locked = setup_state();
        state_mark_locked["mark"]["state"] = json!("locked");
        state_mark_locked["mark"]["proposal_id"] = json!("prop-42");
        state_mark_locked["remaining_required_steps"] = json!(["thinking"]);
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Ok(state_mark_locked),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Thinking);

        // One Continue saves the stated local choice, then finalizes only its admitted result.
        let step = reduce(
            &session,
            &Event::ContinueThinking {
                choice: "local".into(),
                timezone: Some("America/Denver".into()),
            },
        );
        session = step.session;
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::SaveThinking {
                choice: "local".into(),
            })]
        );

        let mut state_thinking_saved = setup_state();
        state_thinking_saved["mark"]["state"] = json!("locked");
        state_thinking_saved["mark"]["proposal_id"] = json!("prop-42");
        state_thinking_saved["thinking"]["saved"] = json!(true);
        state_thinking_saved["thinking"]["choice"] = json!("local");
        state_thinking_saved["remaining_required_steps"] = json!([]);
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Ok(state_thinking_saved),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Thinking);
        let v = view(&session);
        assert_eq!(v["can_finalize"], true);

        assert_eq!(
            step.effects,
            vec![Effect::RecordPending {
                generation: session.inflight.unwrap(),
                timezone: Some("America/Denver".into()),
            }]
        );
        assert!(session.continue_after_save.is_none());

        // Failed finalize stays off Home
        let failed_step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Err(ActionErrorPayload {
                    reason_code: Some("failed".into()),
                    message: "finalize refused".into(),
                    is_transport: false,
                }),
            },
        );
        assert_eq!(failed_step.session.phase, Phase::Thinking);
        assert_eq!(
            failed_step.session.error.as_deref(),
            Some("finalize refused")
        );

        let mut state_done = setup_state();
        state_done["done"] = json!(true);
        state_done["mark"]["state"] = json!("locked");
        state_done["mark"]["proposal_id"] = json!("prop-42");
        state_done["thinking"]["saved"] = json!(true);
        state_done["thinking"]["choice"] = json!("local");
        state_done["remaining_required_steps"] = json!([]);
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Ok(state_done),
            },
        );
        assert_eq!(step.browser_effect(), BrowserEffect::None);
        assert_eq!(
            step.effects,
            vec![Effect::ObserveReceipt {
                generation: step.session.generation,
            }]
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Home);
    }

    #[test]
    fn argv_fresh_on_fresh_off_resumed_no_skip_service() {
        let p = Path::new(r"C:\journal");
        let on = setup_argv(p, false, true);
        assert_eq!(
            on,
            vec![
                "setup",
                "--jsonl",
                "--yes",
                "--accept-existing-journal",
                "--skip-models",
                "--skip-brain",
                "--initial-start-at-sign-in",
                "on",
                "--journal",
                r"C:\journal",
            ]
        );
        assert!(!on.iter().any(|arg| arg == "--skip-service"));

        let off = setup_argv(p, false, false);
        assert_eq!(
            off,
            vec![
                "setup",
                "--jsonl",
                "--yes",
                "--accept-existing-journal",
                "--skip-models",
                "--skip-brain",
                "--initial-start-at-sign-in",
                "off",
                "--journal",
                r"C:\journal",
            ]
        );
        assert!(!off.iter().any(|arg| arg == "--skip-service"));

        let resumed = setup_argv(p, true, false);
        assert_eq!(
            resumed,
            vec![
                "setup",
                "--jsonl",
                "--yes",
                "--accept-existing-journal",
                "--skip-models",
                "--skip-brain",
                "--journal",
                r"C:\journal",
            ]
        );
        assert!(!resumed.iter().any(|arg| arg == "--skip-service"));
    }

    #[test]
    fn stale_generation_duplicate_click_conflict_and_cancel_handling() {
        let mut session = Session {
            generation: 10,
            inflight: Some(10),
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(json!({
                "mark": { "state": "proposed", "proposal_id": "click-id", "render": sample_render() },
                "downloads": { "thinking": { "state": "downloading", "attempt_id": "att-99" } }
            })),
            ..Default::default()
        };

        let step = reduce(
            &session,
            &Event::CommitMarkClicked {
                proposal_id: "click-id".into(),
            },
        );
        assert_eq!(step.session, session);
        assert!(step.effects.is_empty());

        // Worker result from older generation is dropped
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: 9,
                result: Ok(json!({"done": true})),
            },
        );
        assert_eq!(step.session, session);
        assert!(step.effects.is_empty());

        session.inflight = None;
        let step = reduce(
            &session,
            &Event::CommitMarkClicked {
                proposal_id: "click-id".into(),
            },
        );
        session = step.session;
        assert_eq!(session.inflight, Some(11));
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::CommitMark {
                proposal_id: "click-id".into(),
            })]
        );

        // Conflict response emits FetchState and does not commit
        let conflict_step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: 11,
                result: Err(ActionErrorPayload {
                    reason_code: Some("proposal_conflict".into()),
                    message: "identity proposal conflict".into(),
                    is_transport: false,
                }),
            },
        );
        assert_eq!(
            conflict_step.session.error.as_deref(),
            Some("identity proposal conflict")
        );
        assert_eq!(conflict_step.effects, vec![Effect::FetchState]);
        assert_eq!(conflict_step.session.inflight, Some(12));

        session.inflight = None;
        let cancel_step = reduce(
            &session,
            &Event::CancelThinkingDownloadClicked {
                attempt_id: "att-99".into(),
            },
        );
        assert_eq!(
            cancel_step.effects,
            vec![Effect::PostAction(SetupAction::CancelThinkingDownload {
                attempt_id: "att-99".into(),
            })]
        );
    }

    #[test]
    fn saved_local_survives_failures_retry_and_finalize_with_downloads() {
        let mut session = Session {
            phase: Phase::Thinking,
            authoritative: true,
            snapshot: Some(json!({
                "done": false,
                "redirect": "/app/home",
                "mark": { "state": "locked", "proposal_id": "locked-1" },
                "thinking": { "saved": true, "choice": "local", "capability": "ok" },
                "downloads": {
                    "speech": { "state": "complete" },
                    "thinking": { "state": "error", "error": { "code": "dl_failed", "detail": "net" } }
                },
                "remaining_required_steps": []
            })),
            ..Default::default()
        };

        let v = view(&session);
        assert_eq!(v["thinking_retry"], true);
        assert_eq!(v["can_finalize"], true);

        // Missing remaining_required_steps denies finalize
        let missing_steps_session = Session {
            phase: Phase::Thinking,
            snapshot: Some(json!({
                "done": false,
                "redirect": "/app/home",
                "mark": { "state": "locked", "proposal_id": "locked-1" },
                "thinking": { "saved": true, "choice": "local", "capability": "ok" },
                "downloads": {
                    "speech": { "state": "complete" },
                    "thinking": { "state": "error" }
                }
            })),
            ..Default::default()
        };
        assert_eq!(view(&missing_steps_session)["can_finalize"], false);
        let denied_step = reduce(
            &missing_steps_session,
            &Event::FinalizeClicked { timezone: None },
        );
        assert!(denied_step.effects.is_empty());

        // Retry posts start_thinking_download
        let step = reduce(&session, &Event::StartThinkingDownloadClicked);
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::StartThinkingDownload)]
        );

        // Saved byo, and saved local with no thinking state, -> thinking_retry false and click emits nothing
        let byo_session = Session {
            phase: Phase::Thinking,
            snapshot: Some(json!({
                "thinking": { "saved": true, "choice": "byo", "capability": "ok" },
                "downloads": { "thinking": { "state": "error" } }
            })),
            ..Default::default()
        };
        assert_eq!(view(&byo_session)["thinking_retry"], false);
        let byo_step = reduce(&byo_session, &Event::StartThinkingDownloadClicked);
        assert!(byo_step.effects.is_empty());

        let no_state_session = Session {
            phase: Phase::Thinking,
            snapshot: Some(json!({
                "thinking": { "saved": true, "choice": "local", "capability": "ok" },
                "downloads": {}
            })),
            ..Default::default()
        };
        assert_eq!(view(&no_state_session)["thinking_retry"], false);
        let no_state_step = reduce(&no_state_session, &Event::StartThinkingDownloadClicked);
        assert!(no_state_step.effects.is_empty());

        // Finalize during downloading, error, or cancelled succeeds to Home
        session.inflight = None;
        let step = reduce(&session, &Event::FinalizeClicked { timezone: None });
        session = step.session;
        assert_eq!(
            step.effects,
            vec![Effect::RecordPending {
                generation: session.inflight.unwrap(),
                timezone: None,
            }]
        );

        session.models_notice = Some("bundled asset verification failed: wespeaker".into());
        let mut state_done_with_downloads = setup_state();
        state_done_with_downloads["done"] = json!(true);
        state_done_with_downloads["mark"]["state"] = json!("locked");
        state_done_with_downloads["mark"]["proposal_id"] = json!("locked-1");
        state_done_with_downloads["thinking"]["saved"] = json!(true);
        state_done_with_downloads["thinking"]["choice"] = json!("local");
        state_done_with_downloads["remaining_required_steps"] = json!([]);
        state_done_with_downloads["downloads"]["speech"]["state"] = json!("complete");
        state_done_with_downloads["downloads"]["thinking"]["state"] = json!("downloading");
        state_done_with_downloads["downloads"]["thinking"]["attempt_id"] = json!("att-1");
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: session.inflight.unwrap(),
                result: Ok(state_done_with_downloads),
            },
        );
        session = step.session;
        assert_eq!(session.phase, Phase::Home);
        // Speech complete on that home does not clear models_notice
        assert_eq!(
            session.models_notice.as_deref(),
            Some("bundled asset verification failed: wespeaker")
        );
        let v = view(&session);
        assert_eq!(v["downloads"]["speech"]["state"], "complete");
        assert_eq!(v["downloads"]["thinking"]["state"], "downloading");
    }

    #[test]
    fn resume_behavior_not_installed_is_welcome_locked_mark_no_regen_done_is_home() {
        // Not installed -> Welcome, FetchCopy
        let step = reduce(
            &Session::default(),
            &Event::Boot {
                task_installed: false,
            },
        );
        assert_eq!(step.session.phase, Phase::Welcome);
        assert_eq!(step.effects, vec![Effect::FetchCopy]);

        // Installed -> Opening, FetchState, browser None
        let step = reduce(
            &Session::default(),
            &Event::Boot {
                task_installed: true,
            },
        );
        assert_eq!(step.session.phase, Phase::Opening);
        assert_eq!(step.effects, vec![Effect::FetchState]);
        assert_eq!(step.browser_effect(), BrowserEffect::None);

        let mut state_locked_no_regen = setup_state();
        state_locked_no_regen["mark"]["state"] = json!("locked");
        state_locked_no_regen["mark"]["proposal_id"] = json!("existing-locked");
        state_locked_no_regen["remaining_required_steps"] = json!(["thinking"]);
        let step = reduce(
            &step.session,
            &Event::StateLoaded {
                generation: step.session.inflight.unwrap(),
                result: Ok(state_locked_no_regen),
            },
        );
        assert_eq!(step.session.phase, Phase::Thinking);
        assert!(step.effects.is_empty());
        assert!(step.session.inflight.is_none());

        let mut state_done = setup_state();
        state_done["done"] = json!(true);
        state_done["mark"]["state"] = json!("locked");
        state_done["mark"]["proposal_id"] = json!("existing-locked");
        state_done["thinking"]["saved"] = json!(true);
        state_done["thinking"]["choice"] = json!("later");
        state_done["remaining_required_steps"] = json!([]);
        let step = reduce(
            &Session {
                inflight: Some(5),
                ..Default::default()
            },
            &Event::StateLoaded {
                generation: 5,
                result: Ok(state_done),
            },
        );
        assert_eq!(step.session.phase, Phase::Home);
        assert_eq!(step.browser_effect(), BrowserEffect::None);
        assert!(
            !step
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_)))
        );

        let mut evil_done = setup_state();
        evil_done["done"] = json!(true);
        evil_done["redirect"] = json!("https://evil.example/app/home");
        evil_done["remaining_required_steps"] = json!([]);
        let kept = setup_state();
        let evil_step = reduce(
            &Session {
                inflight: Some(6),
                phase: Phase::Opening,
                snapshot: Some(kept.clone()),
                copy: Some(json!({ "kept": true })),
                stored_redirect: Some(APP_HOME_PATH.to_owned()),
                ..Default::default()
            },
            &Event::StateLoaded {
                generation: 6,
                result: Ok(evil_done),
            },
        );
        assert_ne!(evil_step.session.phase, Phase::Home);
        assert_eq!(evil_step.session.snapshot.as_ref(), Some(&kept));
        assert_eq!(evil_step.session.copy, Some(json!({ "kept": true })));
        assert_eq!(
            evil_step.session.stored_redirect.as_deref(),
            Some(APP_HOME_PATH)
        );
        assert!(evil_step.effects.is_empty());
        assert!(evil_step.session.error.is_some());
        assert_eq!(evil_step.browser_effect(), BrowserEffect::None);
    }

    #[test]
    fn browser_effects_spy_enforcement() {
        let step_boot = reduce(
            &Session::default(),
            &Event::Boot {
                task_installed: false,
            },
        );
        assert_eq!(step_boot.browser_effect(), BrowserEffect::None);

        let mut session = Session {
            phase: Phase::Home,
            stored_redirect: Some("/app/home".into()),
            ..Default::default()
        };

        // Open home when stored redirect is /app/home
        let step_open = reduce(&session, &Event::OpenJournalClicked);
        assert_eq!(step_open.browser_effect(), BrowserEffect::OpenHome);
        assert_eq!(
            step_open.effects,
            vec![Effect::Browser(BrowserEffect::OpenHome)]
        );

        // Open home when stored redirect is unexpected
        session.stored_redirect = Some("https://evil.example/app/home".into());
        let step_rejected = reduce(&session, &Event::OpenJournalClicked);
        assert_eq!(step_rejected.browser_effect(), BrowserEffect::Rejected);
        assert_eq!(
            step_rejected.effects,
            vec![Effect::Browser(BrowserEffect::Rejected)]
        );
        assert_eq!(
            step_rejected.session.error.as_deref(),
            Some("unexpected redirect destination")
        );
    }

    #[test]
    fn speech_diagnostics_isolation_and_package_repair() {
        let session = Session {
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(json!({
                "mark": { "state": "proposed", "proposal_id": "p-1", "render": sample_render() },
                "downloads": {
                    "speech": {
                        "state": "error",
                        "error": { "code": "speech_package_unavailable", "detail": "repair or reinstall" }
                    }
                }
            })),
            ..Default::default()
        };

        let v = view(&session);
        assert_eq!(v["speech_retry"], false);

        // Retry clicked does nothing for speech_package_unavailable
        let step = reduce(&session, &Event::RetrySpeechDownloadClicked);
        assert!(step.effects.is_empty());

        let step = reduce(
            &session,
            &Event::CommitMarkClicked {
                proposal_id: "p-1".into(),
            },
        );
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::CommitMark {
                proposal_id: "p-1".into()
            })]
        );

        // Speech complete click emits nothing
        let complete_session = Session {
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(json!({
                "mark": { "state": "proposed", "proposal_id": "p-1", "render": sample_render() },
                "downloads": { "speech": { "state": "complete" } }
            })),
            ..Default::default()
        };
        assert_eq!(view(&complete_session)["speech_retry"], false);
        let step_complete = reduce(&complete_session, &Event::RetrySpeechDownloadClicked);
        assert!(step_complete.effects.is_empty());

        // Another speech error enables retry
        let session_other_err = Session {
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(json!({
                "mark": { "state": "proposed", "proposal_id": "p-1", "render": sample_render() },
                "downloads": {
                    "speech": {
                        "state": "error",
                        "error": { "code": "network_down", "detail": "timeout" }
                    }
                }
            })),
            ..Default::default()
        };
        let v2 = view(&session_other_err);
        assert_eq!(v2["speech_retry"], true);
        let step_retry = reduce(&session_other_err, &Event::RetrySpeechDownloadClicked);
        assert_eq!(
            step_retry.effects,
            vec![Effect::PostAction(SetupAction::RetrySpeechDownload)]
        );
    }

    #[test]
    fn models_diagnostics_lifecycle_and_independence() {
        let mut session = Session {
            models_generation: 1,
            ..Default::default()
        };

        // Check error displays string unchanged
        let step = reduce(
            &session,
            &Event::ModelsCheckFinished {
                generation: 1,
                result: Err("bundled asset verification failed: wespeaker".into()),
            },
        );
        session = step.session;
        assert_eq!(
            session.models_notice.as_deref(),
            Some("bundled asset verification failed: wespeaker")
        );
        assert_eq!(view(&session)["models"]["showing"], true);

        // Speech complete state load whose generation matches inflight leaves notice in place
        session.inflight = Some(10);
        let mut speech_complete = setup_state();
        speech_complete["downloads"]["speech"]["state"] = json!("complete");
        let step = reduce(
            &session,
            &Event::StateLoaded {
                generation: 10,
                result: Ok(speech_complete),
            },
        );
        assert_eq!(
            step.session.models_notice.as_deref(),
            Some("bundled asset verification failed: wespeaker")
        );

        // Install error replaces the string and stays showing
        session.models_generation = 2;
        let step = reduce(
            &session,
            &Event::ModelsInstallFinished {
                generation: 2,
                result: Err(
                    "object detection is unavailable. repair or reinstall your journal.".into(),
                ),
            },
        );
        session = step.session;
        assert_eq!(
            session.models_notice.as_deref(),
            Some("object detection is unavailable. repair or reinstall your journal.")
        );
        assert_eq!(view(&session)["models"]["showing"], true);

        // Install ok does NOT clear and emits CheckModels at the new generation
        let step = reduce(
            &session,
            &Event::ModelsInstallFinished {
                generation: 2,
                result: Ok(()),
            },
        );
        session = step.session;
        assert_eq!(session.models_generation, 3);
        assert_eq!(step.effects, vec![Effect::CheckModels { generation: 3 }]);
        assert!(session.models_notice.is_some());

        // Stale check result does not clear it
        let stale_step = reduce(
            &session,
            &Event::ModelsCheckFinished {
                generation: 2,
                result: Ok(()),
            },
        );
        assert_eq!(
            stale_step.session.models_notice.as_deref(),
            session.models_notice.as_deref()
        );

        // Later check success clears it
        let step = reduce(
            &session,
            &Event::ModelsCheckFinished {
                generation: 3,
                result: Ok(()),
            },
        );
        session = step.session;
        assert_eq!(session.models_notice, None);
        assert_eq!(view(&session)["models"]["showing"], false);
    }

    #[test]
    fn allowlist_origin_parser_and_hostile_error_handling() {
        // conveys allowed
        assert!(convey_route_allowed("GET", "/init/api/state"));
        assert!(convey_route_allowed("POST", "/init/api/action"));
        assert!(convey_route_allowed("GET", "/app/settings/api/config"));
        assert!(convey_route_allowed("PUT", "/app/settings/api/config"));
        assert!(convey_route_allowed("POST", "/app/network/unpair"));

        // conveys rejected
        assert!(!convey_route_allowed("POST", "/init/api/state"));
        assert!(!convey_route_allowed("GET", "/init/api/action"));
        assert!(!convey_route_allowed("GET", "/init/mark"));
        assert!(!convey_route_allowed("POST", "/init/mark/regenerate"));
        assert!(!convey_route_allowed("POST", "/init/mark/lock"));
        assert!(!convey_route_allowed("POST", "/init/finalize"));
        assert!(!convey_route_allowed("GET", "/init/api/state?q=1"));
        assert!(!convey_route_allowed("GET", "/init/api/state#sec"));
        assert!(!convey_route_allowed(
            "GET",
            "/app/settings/api/config/../../etc"
        ));

        // is_app_origin allowed
        assert!(is_app_origin("https://journal.localhost"));
        assert!(is_app_origin("https://journal.localhost/"));
        assert!(is_app_origin("https://journal.localhost:443"));
        assert!(is_app_origin("https://journal.localhost:443/"));
        assert!(is_app_origin("https://journal.localhost:443/index.html"));
        assert!(is_app_origin("https://journal.localhost/app.js?v=1#anchor"));
        assert!(is_app_origin("HTTPS://JOURNAL.LOCALHOST/INDEX.HTML"));

        // is_app_origin rejected
        assert!(!is_app_origin("https://journal.localhost:444"));
        assert!(!is_app_origin("https://journal.localhost:4433"));
        assert!(!is_app_origin("https://journal.localhost.evil"));
        assert!(!is_app_origin("https://journal.localhost.evil/index.html"));
        assert!(!is_app_origin("https://user@journal.localhost"));
        assert!(!is_app_origin("http://journal.localhost"));
        assert!(!is_app_origin("http://journal.localhost:443"));
        assert!(!is_app_origin(""));
        assert!(!is_app_origin("not-a-url"));

        // app_asset_path cases
        assert_eq!(
            app_asset_path("https://journal.localhost:443/app.js").as_deref(),
            Some("/app.js")
        );
        assert_eq!(
            app_asset_path("https://journal.localhost/app.js?v=1#a").as_deref(),
            Some("/app.js")
        );
        assert_eq!(
            app_asset_path("https://journal.localhost.evil/app.js"),
            None
        );
        assert_eq!(
            app_asset_path("https://user@journal.localhost/app.js"),
            None
        );
        assert_eq!(
            app_asset_path("https://journal.localhost").as_deref(),
            Some("/")
        );
        assert_eq!(
            app_asset_path("https://journal.localhost:443").as_deref(),
            Some("/")
        );
        assert_eq!(
            app_asset_path("https://journal.localhost/").as_deref(),
            Some("/")
        );
        let malformed = "https://journal.localhost:not-a-port";
        assert!(url::Url::parse(malformed).is_err());
        assert!(!is_app_origin(malformed));
        assert_eq!(app_asset_path(malformed), None);

        // Hostile error string
        let session = Session {
            inflight: Some(1),
            ..Default::default()
        };
        let step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: 1,
                result: Err(ActionErrorPayload {
                    reason_code: None,
                    message: "<script>alert('xss')</script>".into(),
                    is_transport: false,
                }),
            },
        );
        assert_eq!(
            step.session.error.as_deref(),
            Some("<script>alert('xss')</script>")
        );
        assert_ne!(step.session.phase, Phase::Home);
        assert!(
            step.effects
                .iter()
                .any(|effect| matches!(effect, Effect::FetchState))
        );
        assert!(
            !step
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::Browser(_)))
        );
    }

    #[test]
    fn choose_thinking_local_capability_gate() {
        let blocked_session = Session {
            phase: Phase::Thinking,
            authoritative: true,
            snapshot: Some(json!({
                "thinking": { "capability": "blocked" }
            })),
            ..Default::default()
        };
        let step = reduce(&blocked_session, &Event::ChooseThinking("local".into()));
        assert!(step.effects.is_empty());
        assert!(step.session.error.is_some());

        let ok_session = Session {
            phase: Phase::Thinking,
            authoritative: true,
            snapshot: Some(json!({
                "thinking": { "capability": "ok" }
            })),
            ..Default::default()
        };
        let step = reduce(&ok_session, &Event::ChooseThinking("local".into()));
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::SaveThinking {
                choice: "local".into()
            })]
        );

        let unknown_session = Session {
            phase: Phase::Thinking,
            authoritative: true,
            snapshot: Some(json!({
                "thinking": { "capability": "unknown" }
            })),
            ..Default::default()
        };
        let step = reduce(&unknown_session, &Event::ChooseThinking("local".into()));
        assert!(step.effects.is_empty());
        assert!(step.session.error.is_some());
    }

    #[test]
    fn retry_state_policy() {
        // While inflight is set, ignored
        let inflight_session = Session {
            inflight: Some(1),
            ..Default::default()
        };
        let step = reduce(&inflight_session, &Event::RetryState);
        assert_eq!(step.session, inflight_session);
        assert!(step.effects.is_empty());

        // While phase is Setup, !setup_failed, and setup_running, ignored
        let running_setup_session = Session {
            phase: Phase::Setup,
            setup_failed: false,
            setup_running: true,
            ..Default::default()
        };
        let step = reduce(&running_setup_session, &Event::RetryState);
        assert_eq!(step.session, running_setup_session);
        assert!(step.effects.is_empty());

        // When setup failed, or state read failed, emits FetchState
        let failed_setup_session = Session {
            phase: Phase::Setup,
            setup_failed: true,
            setup_running: false,
            ..Default::default()
        };
        let step = reduce(&failed_setup_session, &Event::RetryState);
        assert_eq!(step.effects, vec![Effect::FetchState]);
        assert_eq!(step.session.inflight, Some(1));

        let opening_session = Session {
            phase: Phase::Opening,
            ..Default::default()
        };
        let step = reduce(&opening_session, &Event::RetryState);
        assert_eq!(step.effects, vec![Effect::FetchState]);
        assert_eq!(step.session.inflight, Some(1));
    }

    fn load(session: &Session, state: Value) -> Step {
        reduce(
            session,
            &Event::StateLoaded {
                generation: session.inflight.unwrap_or(1),
                result: Ok(state),
            },
        )
    }

    #[test]
    fn painted_ids_are_posted_and_blank_ids_do_not() {
        let mut snapshot = setup_state();
        snapshot["mark"]["state"] = json!("proposed");
        snapshot["mark"]["proposal_id"] = json!("Y");
        snapshot["mark"]["render"] = sample_render();
        snapshot["downloads"]["thinking"]["state"] = json!("downloading");
        snapshot["downloads"]["thinking"]["attempt_id"] = json!("B");
        let session = Session {
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(snapshot),
            ..Default::default()
        };
        let commit = event_from_intent("commit-mark", &json!({ "proposal_id": "X" })).unwrap();
        let step = reduce(&session, &commit);
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::CommitMark {
                proposal_id: "X".into(),
            })]
        );
        let cancel =
            event_from_intent("cancel-thinking-download", &json!({ "attempt_id": "A" })).unwrap();
        let step = reduce(&session, &cancel);
        assert_eq!(
            step.effects,
            vec![Effect::PostAction(SetupAction::CancelThinkingDownload {
                attempt_id: "A".into(),
            })]
        );
        for args in [
            json!({}),
            json!({ "proposal_id": "" }),
            json!({ "proposal_id": "  " }),
        ] {
            let event = event_from_intent("commit-mark", &args).unwrap();
            let step = reduce(&session, &event);
            assert!(step.session.error.is_some());
            assert!(step.effects.is_empty());
            assert_eq!(step.session.generation, session.generation);
        }
        for args in [json!({}), json!({ "attempt_id": " \n" })] {
            let event = event_from_intent("cancel-thinking-download", &args).unwrap();
            let step = reduce(&session, &event);
            assert!(step.session.error.is_some());
            assert!(step.effects.is_empty());
            assert_eq!(step.session.generation, session.generation);
        }
        assert!(event_from_intent("boot", &json!({})).is_err());
    }

    fn rejected_state_keeps_snapshot(bad: Value) {
        let previous = setup_state();
        let session = Session {
            phase: Phase::Folder,
            snapshot: Some(previous.clone()),
            copy: Some(json!({ "kept": true })),
            stored_redirect: Some(APP_HOME_PATH.to_owned()),
            inflight: Some(2),
            ..Default::default()
        };
        let step = load(&session, bad);
        assert_eq!(step.session.snapshot.as_ref(), Some(&previous));
        assert_eq!(step.session.copy, Some(json!({ "kept": true })));
        assert_eq!(step.session.stored_redirect.as_deref(), Some(APP_HOME_PATH));
        assert!(step.effects.is_empty());
        assert!(step.session.error.is_some());
        assert_ne!(step.session.phase, Phase::Home);
    }

    #[test]
    fn inconsistent_setup_state_is_not_admitted() {
        rejected_state_keeps_snapshot(Value::Null);
        let mut wrong = setup_state();
        wrong["schema"] = json!("other");
        rejected_state_keeps_snapshot(wrong);
        let mut missing_mark = setup_state();
        missing_mark.as_object_mut().unwrap().remove("mark");
        rejected_state_keeps_snapshot(missing_mark);
        let mut saved_without_choice = setup_state();
        saved_without_choice["thinking"]["saved"] = json!(true);
        rejected_state_keeps_snapshot(saved_without_choice);
        let mut unsaved_choice = setup_state();
        unsaved_choice["thinking"]["choice"] = json!("local");
        rejected_state_keeps_snapshot(unsaved_choice);
        let mut proposed = setup_state();
        proposed["mark"]["state"] = json!("proposed");
        rejected_state_keeps_snapshot(proposed);
        let mut done_remaining = setup_state();
        done_remaining["done"] = json!(true);
        done_remaining["remaining_required_steps"] = json!(["mark"]);
        rejected_state_keeps_snapshot(done_remaining);
        let mut foreign = setup_state();
        foreign["done"] = json!(true);
        foreign["remaining_required_steps"] = json!([]);
        foreign["redirect"] = json!("https://evil.example/app/home");
        rejected_state_keeps_snapshot(foreign);

        let mut extra = setup_state();
        extra["version"] = json!(3);
        extra["lanes"] = json!(["confidential"]);
        extra["mark"]["state"] = json!("proposed");
        extra["mark"]["proposal_id"] = json!("kept-id");
        extra["mark"]["render"] = sample_render();
        let step = load(
            &Session {
                inflight: Some(1),
                resting: true,
                phase: Phase::Opening,
                ..Default::default()
            },
            extra,
        );
        assert_eq!(step.session.phase, Phase::Mark);
        assert!(step.effects.is_empty());
        assert!(!step.session.resting);
        assert_eq!(
            step.session
                .snapshot
                .as_ref()
                .and_then(|state| state.pointer("/mark/proposal_id"))
                .and_then(Value::as_str),
            Some("kept-id")
        );
    }

    #[test]
    fn reads_do_not_regenerate_mark() {
        let fresh = load(
            &Session {
                inflight: Some(1),
                phase: Phase::Opening,
                ..Default::default()
            },
            setup_state(),
        );
        assert_eq!(fresh.session.phase, Phase::Mark);
        assert!(fresh.effects.is_empty());

        let mut proposed = setup_state();
        proposed["mark"]["state"] = json!("proposed");
        proposed["mark"]["proposal_id"] = json!("p-1");
        proposed["mark"]["render"] = sample_render();
        let step = load(
            &Session {
                inflight: Some(1),
                ..Default::default()
            },
            proposed.clone(),
        );
        assert_eq!(step.session.phase, Phase::Mark);
        assert!(step.effects.is_empty());

        let mut locked = setup_state();
        locked["mark"]["state"] = json!("locked");
        locked["remaining_required_steps"] = json!(["thinking"]);
        let step = load(
            &Session {
                inflight: Some(1),
                ..Default::default()
            },
            locked.clone(),
        );
        assert_eq!(step.session.phase, Phase::Thinking);
        assert!(step.effects.is_empty());

        let mut saved = locked;
        saved["thinking"]["saved"] = json!(true);
        saved["thinking"]["choice"] = json!("later");
        let step = load(
            &Session {
                inflight: Some(1),
                ..Default::default()
            },
            saved,
        );
        assert_eq!(step.session.phase, Phase::Thinking);
        assert!(step.effects.is_empty());

        let session = Session {
            phase: Phase::Mark,
            authoritative: true,
            snapshot: Some(proposed),
            ..Default::default()
        };
        let clicked = reduce(&session, &Event::RegenerateMarkClicked);
        assert_eq!(
            clicked.effects,
            vec![Effect::PostAction(SetupAction::RegenerateMark)]
        );
        let locked_session = Session {
            phase: Phase::Mark,
            snapshot: Some({
                let mut state = setup_state();
                state["mark"]["state"] = json!("locked");
                state
            }),
            ..Default::default()
        };
        let ignored = reduce(&locked_session, &Event::RegenerateMarkClicked);
        assert!(ignored.effects.is_empty());
    }

    #[test]
    fn legacy_complete_and_later_home_keep_one_models_check() {
        let mut legacy = setup_state();
        legacy["done"] = json!(true);
        legacy["remaining_required_steps"] = json!([]);
        let step = load(
            &Session {
                inflight: Some(1),
                phase: Phase::Opening,
                ..Default::default()
            },
            legacy,
        );
        assert_eq!(step.session.phase, Phase::Home);
        assert_eq!(step.browser_effect(), BrowserEffect::None);
        assert!(
            !step
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::PostAction(_) | Effect::Browser(_)))
        );
        assert!(
            step.effects
                .iter()
                .any(|effect| matches!(effect, Effect::CheckModels { .. }))
        );

        let mut later = setup_state();
        later["done"] = json!(true);
        later["remaining_required_steps"] = json!([]);
        later["downloads"]["speech"]["state"] = json!("complete");
        later["downloads"]["thinking"]["state"] = json!("complete");
        let session = Session {
            phase: Phase::Home,
            models_notice: Some("keep".into()),
            models_pending: true,
            models_generation: 4,
            inflight: Some(1),
            ..Default::default()
        };
        let step = load(&session, later);
        assert_eq!(step.session.phase, Phase::Home);
        assert!(step.effects.is_empty());
        assert_eq!(step.session.models_notice.as_deref(), Some("keep"));
        assert!(step.session.models_pending);
        assert_eq!(step.session.models_generation, 4);
    }

    #[test]
    fn action_errors_refresh_and_keep_the_message() {
        let mut body = setup_state();
        body["mark"]["state"] = json!("proposed");
        body["mark"]["proposal_id"] = json!("Y");
        body["mark"]["render"] = sample_render();
        let session = Session {
            phase: Phase::Thinking,
            inflight: Some(3),
            snapshot: Some(body.clone()),
            ..Default::default()
        };
        for (code, message) in [
            ("start_failed", "could not start"),
            ("proposal_conflict", "conflict"),
        ] {
            let step = reduce(
                &session,
                &Event::ActionCompleted {
                    generation: 3,
                    result: Err(ActionErrorPayload {
                        reason_code: Some(code.into()),
                        message: message.into(),
                        is_transport: false,
                    }),
                },
            );
            assert_eq!(step.effects, vec![Effect::FetchState]);
            assert_eq!(step.session.error.as_deref(), Some(message));
            assert!(!step.effects.iter().any(|effect| matches!(
                effect,
                Effect::PostAction(SetupAction::CommitMark { .. }) | Effect::Browser(_)
            )));
            assert_ne!(step.session.phase, Phase::Home);
            let follow = reduce(
                &step.session,
                &Event::StateLoaded {
                    generation: step.session.inflight.unwrap(),
                    result: Ok(body.clone()),
                },
            );
            assert_eq!(follow.session.error.as_deref(), Some(message));
            assert_ne!(follow.session.phase, Phase::Home);
        }
    }

    #[test]
    fn withhold_remote_uses_the_bound_port_or_drops_journal_calls() {
        let mut session = Session::default();
        let mut effects = vec![Effect::FetchState, Effect::Browser(BrowserEffect::OpenHome)];
        let kept = effects.clone();
        let port = withhold_remote(&mut session, &mut effects, Some(5016)).unwrap();
        assert_eq!(port, 5016);
        assert_eq!(effects, kept);
        assert_eq!(home_url(5016), "http://127.0.0.1:5016/app/home");

        let mut session = Session {
            phase: Phase::Opening,
            task_installed: true,
            inflight: Some(4),
            busy: true,
            ..Default::default()
        };
        let mut effects = vec![
            Effect::FetchState,
            Effect::Browser(BrowserEffect::OpenHome),
            Effect::CheckModels { generation: 3 },
        ];
        assert!(withhold_remote(&mut session, &mut effects, None).is_err());
        assert_eq!(effects, vec![Effect::CheckModels { generation: 3 }]);
        assert!(session.error.is_some());
        assert!(session.resting);
        assert_ne!(session.phase, Phase::Home);
        assert!(!session.busy);
        assert!(session.inflight.is_none());
        assert_eq!(view(&session)["resting"], true);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Browser(_)))
        );

        let mut local = vec![Effect::CheckModels { generation: 1 }];
        let mut idle = Session::default();
        assert_eq!(withhold_remote(&mut idle, &mut local, None).unwrap(), 0);
        assert_eq!(local, vec![Effect::CheckModels { generation: 1 }]);
        assert!(idle.error.is_none());
    }

    #[test]
    fn models_retry_does_not_start_a_second_flight() {
        let first = reduce(&Session::default(), &Event::RetryModelsInstallClicked);
        assert_eq!(first.effects, vec![Effect::InstallModels { generation: 1 }]);
        assert!(first.session.models_pending);
        let second = reduce(&first.session, &Event::RetryModelsInstallClicked);
        assert!(second.effects.is_empty());
        assert_eq!(
            second.session.models_generation,
            first.session.models_generation
        );
    }

    #[test]
    fn execution_order_replies_before_effects() {
        let items = execution_order(&[Effect::FetchCopy, Effect::FetchState]);
        assert_eq!(
            items,
            vec![
                TurnItem::Reply,
                TurnItem::Effect(Effect::FetchCopy),
                TurnItem::Effect(Effect::FetchState),
            ]
        );
    }

    #[test]
    fn view_step_follows_the_setup_sequence() {
        for (phase, step) in [
            (Phase::Welcome, Some(1)),
            (Phase::SignIn, Some(1)),
            (Phase::Folder, Some(2)),
            (Phase::Mark, Some(3)),
            (Phase::Thinking, Some(4)),
            (Phase::Home, None),
            (Phase::Opening, None),
            (Phase::Setup, None),
        ] {
            let session = Session {
                phase,
                ..Default::default()
            };
            assert_eq!(view(&session)["step"], json!(step));
        }
    }

    #[test]
    fn view_thinking_download_bytes() {
        let session = Session::default();
        assert_eq!(view(&session)["thinking_download_bytes"], Value::Null);

        let session = Session {
            snapshot: Some(json!({"thinking": {"download_bytes": 1500000}})),
            ..Default::default()
        };
        assert_eq!(view(&session)["thinking_download_bytes"], json!(1500000));

        let session = Session {
            snapshot: Some(json!({"thinking": {}})),
            ..Default::default()
        };
        assert_eq!(view(&session)["thinking_download_bytes"], Value::Null);

        let session = Session {
            snapshot: Some(json!({"thinking": {"download_bytes": "nope"}})),
            ..Default::default()
        };
        assert_eq!(view(&session)["thinking_download_bytes"], Value::Null);
    }

    #[test]
    fn reply_is_issued_before_effects_run() {
        use std::cell::RefCell;
        let log = RefCell::new(Vec::new());
        let rendered = json!({ "phase": "home" });
        let effects = vec![Effect::FetchState];
        execute_turn(
            rendered,
            &effects,
            |_val| log.borrow_mut().push("reply"),
            |_eff| log.borrow_mut().push("effect"),
        );
        assert_eq!(*log.borrow(), vec!["reply", "effect"]);
    }

    #[test]
    fn convey_dispatch_matrix_and_port_rules() {
        // setup GET /init/api/state and POST /init/api/action with status None and renderer Some(5015), Some(80), Some(0), and None are Unanswered.
        for method in ["GET", "POST"] {
            let path = if method == "GET" {
                "/init/api/state"
            } else {
                "/init/api/action"
            };
            for renderer in [Some(5015), Some(80), Some(0), None] {
                assert_eq!(
                    convey_dispatch(method, path, renderer, None),
                    ConveyDispatch::Unanswered
                );
            }
        }
        // Status Some(5016) with renderer Some(5015) is Use(Some(5016)).
        assert_eq!(
            convey_dispatch("GET", "/init/api/state", Some(5015), Some(5016)),
            ConveyDispatch::Use(Some(5016))
        );
        assert_eq!(
            convey_dispatch("POST", "/init/api/action", Some(5015), Some(5016)),
            ConveyDispatch::Use(Some(5016))
        );
        // Settings GET and devices routes with renderer None are Use(None).
        assert_eq!(
            convey_dispatch("GET", "/app/settings/api/config", None, None),
            ConveyDispatch::Use(None)
        );
        assert_eq!(
            convey_dispatch("GET", "/app/network/api/devices", None, None),
            ConveyDispatch::Use(None)
        );
        // Renderer Some(5016) on those routes is Use(Some(5016)).
        assert_eq!(
            convey_dispatch("GET", "/app/settings/api/config", Some(5016), None),
            ConveyDispatch::Use(Some(5016))
        );
        assert_eq!(
            convey_dispatch("GET", "/app/network/api/devices", Some(5016), None),
            ConveyDispatch::Use(Some(5016))
        );
        // A path with ? or .. is Rejected.
        assert_eq!(
            convey_dispatch("GET", "/init/api/state?q=1", Some(5015), Some(5016)),
            ConveyDispatch::Rejected
        );
        assert_eq!(
            convey_dispatch(
                "GET",
                "/app/settings/api/config/../evil",
                Some(5015),
                Some(5016)
            ),
            ConveyDispatch::Rejected
        );
    }

    #[test]
    fn authority_bit_blocks_mutations_and_recovers_on_admitted_refresh() {
        // Include a negative bytes_done and a fractional download_bytes as malformed.
        let mut bad_bytes_done = setup_state();
        bad_bytes_done["downloads"]["speech"]["bytes_done"] = json!(-1);
        assert!(admit_setup_state(&bad_bytes_done).is_err());

        let mut bad_download_bytes = setup_state();
        bad_download_bytes["thinking"]["download_bytes"] = json!(12.5);
        assert!(admit_setup_state(&bad_download_bytes).is_err());

        // valid state sets authority true
        let mut valid_state = setup_state();
        valid_state["mark"]["state"] = json!("proposed");
        valid_state["mark"]["proposal_id"] = json!("prop-1");
        valid_state["mark"]["render"] = sample_render();

        let session = Session {
            inflight: Some(1),
            phase: Phase::Mark,
            ..Default::default()
        };
        let step = reduce(
            &session,
            &Event::StateLoaded {
                generation: 1,
                result: Ok(valid_state.clone()),
            },
        );
        let mut session = step.session;
        assert!(session.authoritative);

        // transport error or malformed state clears authority
        session.inflight = Some(2);
        let step = reduce(
            &session,
            &Event::StateLoaded {
                generation: 2,
                result: Err("transport failure".into()),
            },
        );
        session = step.session;
        assert!(!session.authoritative);

        // each of the seven mutations has no effects
        let m1 = reduce(&session, &Event::RegenerateMarkClicked);
        assert!(m1.effects.is_empty());

        let m2 = reduce(
            &session,
            &Event::CommitMarkClicked {
                proposal_id: "prop-1".into(),
            },
        );
        assert!(m2.effects.is_empty());

        let mut thinking_session = session.clone();
        thinking_session.phase = Phase::Thinking;
        let m3 = reduce(&thinking_session, &Event::ChooseThinking("local".into()));
        assert!(m3.effects.is_empty());

        let m4 = reduce(&thinking_session, &Event::StartThinkingDownloadClicked);
        assert!(m4.effects.is_empty());

        let m5 = reduce(
            &thinking_session,
            &Event::CancelThinkingDownloadClicked {
                attempt_id: "att-1".into(),
            },
        );
        assert!(m5.effects.is_empty());

        let m6 = reduce(&session, &Event::RetrySpeechDownloadClicked);
        assert!(m6.effects.is_empty());

        let m7 = reduce(
            &thinking_session,
            &Event::FinalizeClicked { timezone: None },
        );
        assert!(m7.effects.is_empty());

        // RetryState emits FetchState
        let step = reduce(&session, &Event::RetryState);
        assert_eq!(step.effects, vec![Effect::FetchState]);
        let load_gen = step.session.inflight.unwrap();

        // valid state restores authority and does not emit save/commit/finalize
        let step = reduce(
            &step.session,
            &Event::StateLoaded {
                generation: load_gen,
                result: Ok(valid_state.clone()),
            },
        );
        session = step.session;
        assert!(session.authoritative);
        assert!(
            !step
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_)))
        );

        // one new click emits its effect (empty effect list on recovered click fails test)
        let click_step = reduce(&session, &Event::RegenerateMarkClicked);
        assert_eq!(
            click_step.effects,
            vec![Effect::PostAction(SetupAction::RegenerateMark)]
        );

        // Action-error text remains after a successful follow-up load and after a failed follow-up load
        session.inflight = Some(10);
        let action_err_step = reduce(
            &session,
            &Event::ActionCompleted {
                generation: 10,
                result: Err(ActionErrorPayload {
                    reason_code: Some("err_code".into()),
                    message: "action failed detail".into(),
                    is_transport: false,
                }),
            },
        );
        session = action_err_step.session;
        assert_eq!(session.error.as_deref(), Some("action failed detail"));
        assert_eq!(action_err_step.effects, vec![Effect::FetchState]);

        let success_load = reduce(
            &session,
            &Event::StateLoaded {
                generation: session.inflight.unwrap(),
                result: Ok(valid_state),
            },
        );
        assert_eq!(
            success_load.session.error.as_deref(),
            Some("action failed detail")
        );

        let failed_load = reduce(
            &session,
            &Event::StateLoaded {
                generation: session.inflight.unwrap(),
                result: Err("failed load".into()),
            },
        );
        assert_eq!(
            failed_load.session.error.as_deref(),
            Some("action failed detail")
        );

        // Include legacy done: true, unsaved thinking, choice null, render null: phase Home, no PostAction, no Browser
        let mut legacy = setup_state();
        legacy["done"] = json!(true);
        legacy["remaining_required_steps"] = json!([]);
        legacy["thinking"]["saved"] = json!(false);
        legacy["thinking"]["choice"] = Value::Null;
        // render is null in setup_state()
        let leg_session = Session {
            inflight: Some(20),
            ..Default::default()
        };
        let leg_step = reduce(
            &leg_session,
            &Event::StateLoaded {
                generation: 20,
                result: Ok(legacy),
            },
        );
        assert_eq!(leg_step.session.phase, Phase::Home);
        assert!(
            !leg_step
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_) | Effect::Browser(_)))
        );
    }

    #[test]
    fn models_admission_serializes_checks_and_installs_through_reduce() {
        // install in flight, then RequestModelsCheck twice and a not-yet-home done load:
        // generation unchanged, no extra effect, notice unchanged.
        let mut session = Session {
            phase: Phase::Opening,
            models_generation: 1,
            models_pending: true,
            models_notice: Some("current notice".into()),
            ..Default::default()
        };

        let step1 = reduce(&session, &Event::RequestModelsCheck);
        assert!(step1.effects.is_empty());
        assert_eq!(step1.session.models_generation, 1);
        assert_eq!(
            step1.session.models_notice.as_deref(),
            Some("current notice")
        );

        let step2 = reduce(&step1.session, &Event::RequestModelsCheck);
        assert!(step2.effects.is_empty());
        assert_eq!(step2.session.models_generation, 1);

        session = step2.session;
        session.inflight = Some(5);

        let mut done_state = setup_state();
        done_state["done"] = json!(true);
        done_state["remaining_required_steps"] = json!([]);
        let step_done = reduce(
            &session,
            &Event::StateLoaded {
                generation: 5,
                result: Ok(done_state),
            },
        );
        assert_eq!(step_done.session.phase, Phase::Home);
        assert_eq!(
            step_done.effects,
            vec![Effect::ObserveReceipt {
                generation: step_done.session.generation,
            }]
        );
        assert_eq!(step_done.session.models_generation, 1);
        assert_eq!(
            step_done.session.models_notice.as_deref(),
            Some("current notice")
        );
        session = step_done.session;

        // Stale completion does not emit check
        let stale = reduce(
            &session,
            &Event::ModelsInstallFinished {
                generation: 0,
                result: Ok(()),
            },
        );
        assert!(stale.effects.is_empty());
        assert_eq!(stale.session.models_generation, 1);
        assert!(stale.session.models_pending);

        // Install completion for that generation emits one check
        let install_done = reduce(
            &session,
            &Event::ModelsInstallFinished {
                generation: 1,
                result: Ok(()),
            },
        );
        assert_eq!(
            install_done.effects,
            vec![Effect::CheckModels { generation: 2 }]
        );
        assert_eq!(install_done.session.models_generation, 2);
        assert!(install_done.session.models_pending);
        assert_eq!(
            install_done.session.models_notice.as_deref(),
            Some("current notice")
        );
        session = install_done.session;

        // Check in flight ignores repeated RequestModelsCheck
        let repeat_check = reduce(&session, &Event::RequestModelsCheck);
        assert!(repeat_check.effects.is_empty());
        assert_eq!(repeat_check.session.models_generation, 2);

        // Check Err keeps the text
        let check_err = reduce(
            &session,
            &Event::ModelsCheckFinished {
                generation: 2,
                result: Err("model check failure msg".into()),
            },
        );
        assert_eq!(
            check_err.session.models_notice.as_deref(),
            Some("model check failure msg")
        );
        assert!(!check_err.session.models_pending);
        session = check_err.session;

        // RetryModelsInstallClicked then admits one install
        let retry_install = reduce(&session, &Event::RetryModelsInstallClicked);
        assert_eq!(
            retry_install.effects,
            vec![Effect::InstallModels { generation: 3 }]
        );
        assert_eq!(retry_install.session.models_generation, 3);
        assert!(retry_install.session.models_pending);

        // Finish install and check Ok clears notice
        let install_finish = reduce(
            &retry_install.session,
            &Event::ModelsInstallFinished {
                generation: 3,
                result: Ok(()),
            },
        );
        assert_eq!(
            install_finish.effects,
            vec![Effect::CheckModels { generation: 4 }]
        );

        let check_ok = reduce(
            &install_finish.session,
            &Event::ModelsCheckFinished {
                generation: 4,
                result: Ok(()),
            },
        );
        assert_eq!(check_ok.session.models_notice, None);
        assert!(!check_ok.session.models_pending);
    }

    #[test]
    fn stopped_opening_reconciles_without_a_setup_write() {
        // Opening session, RetryState emits only FetchState
        let session = Session {
            phase: Phase::Opening,
            ..Default::default()
        };
        let step = reduce(&session, &Event::RetryState);
        assert_eq!(step.effects, vec![Effect::FetchState]);
        let load_gen = step.session.inflight.unwrap();

        // Admitted not-done state lands on Mark or Thinking
        let not_done_mark = setup_state();
        let step_mark = reduce(
            &step.session,
            &Event::StateLoaded {
                generation: load_gen,
                result: Ok(not_done_mark),
            },
        );
        assert_eq!(step_mark.session.phase, Phase::Mark);
        assert!(
            !step_mark
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_) | Effect::Browser(_)))
        );

        let mut not_done_thinking = setup_state();
        not_done_thinking["mark"]["state"] = json!("locked");
        not_done_thinking["remaining_required_steps"] = json!(["thinking"]);
        let step_thinking = reduce(
            &step.session,
            &Event::StateLoaded {
                generation: load_gen,
                result: Ok(not_done_thinking),
            },
        );
        assert_eq!(step_thinking.session.phase, Phase::Thinking);
        assert!(
            !step_thinking
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_) | Effect::Browser(_)))
        );

        // Admitted done lands on Home. Neither emits PostAction or Browser.
        let mut done_state = setup_state();
        done_state["done"] = json!(true);
        done_state["remaining_required_steps"] = json!([]);
        let step_home = reduce(
            &step.session,
            &Event::StateLoaded {
                generation: load_gen,
                result: Ok(done_state),
            },
        );
        assert_eq!(step_home.session.phase, Phase::Home);
        assert!(
            !step_home
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PostAction(_) | Effect::Browser(_)))
        );
    }
    #[test]
    fn status_failure_revokes_authority_until_new_state_and_owner_click() {
        let mut session = Session::default();
        let mut effects = vec![];
        let mut state = setup_state();
        state["mark"]["state"] = json!("locked");
        apply_snapshot(&mut session, &mut effects, &state);
        let mut pending = reduce(&session, &Event::RetryState);
        assert!(withhold_remote(&mut pending.session, &mut pending.effects, None).is_err());
        assert!(!pending.session.authoritative);
        assert!(pending.effects.is_empty());
        assert!(
            reduce(&pending.session, &Event::ChooseThinking("local".into()))
                .effects
                .is_empty()
        );
        assert!(
            reduce(&pending.session, &Event::FinalizeClicked { timezone: None })
                .effects
                .is_empty()
        );
        let retry = reduce(&pending.session, &Event::RetryState);
        let recovered = reduce(
            &retry.session,
            &Event::StateLoaded {
                generation: retry.session.inflight.unwrap(),
                result: Ok(state),
            },
        );
        assert!(recovered.session.authoritative);
        assert!(recovered.effects.is_empty());
        assert!(matches!(
            reduce(
                &recovered.session,
                &Event::ContinueThinking {
                    choice: "local".into(),
                    timezone: None,
                }
            )
            .effects
            .as_slice(),
            [Effect::PostAction(SetupAction::SaveThinking { .. })]
        ));
    }

    #[test]
    fn first_mark_intent_preserves_existing_proposals_and_refresh_is_read_only() {
        for mark in ["unavailable", "proposed", "locked"] {
            let mut state = setup_state();
            state["mark"]["state"] = json!(mark);
            if mark != "unavailable" {
                state["mark"]["proposal_id"] = json!("existing-proposal");
                state["mark"]["render"] = sample_render();
            }
            let existing = Session {
                initial_mark_pending: true,
                inflight: Some(1),
                ..Default::default()
            };
            let loaded = reduce(
                &existing,
                &Event::StateLoaded {
                    generation: 1,
                    result: Ok(state.clone()),
                },
            );
            if mark == "unavailable" {
                assert!(matches!(
                    loaded.effects.as_slice(),
                    [Effect::PostAction(SetupAction::RegenerateMark)]
                ));
            } else {
                assert!(loaded.effects.is_empty());
            }
            let ordinary = Session {
                inflight: Some(1),
                ..Default::default()
            };
            let read = reduce(
                &ordinary,
                &Event::StateLoaded {
                    generation: 1,
                    result: Ok(state),
                },
            );
            assert!(read.effects.is_empty());
            assert!(!read.session.initial_mark_pending);
        }
    }

    #[test]
    fn continue_intent_stops_on_error_stale_malformed_or_changed_selection() {
        let mut initial = setup_state();
        initial["mark"]["state"] = json!("locked");
        let mut session = Session::default();
        apply_snapshot(&mut session, &mut vec![], &initial);
        let saving = reduce(
            &session,
            &Event::ContinueThinking {
                choice: "later".into(),
                timezone: None,
            },
        );
        let generation = saving.session.inflight.unwrap();
        let mut saved = initial.clone();
        saved["thinking"]["saved"] = json!(true);
        saved["thinking"]["choice"] = json!("later");
        saved["remaining_required_steps"] = json!([]);
        let stale = reduce(
            &saving.session,
            &Event::ActionCompleted {
                generation: generation + 1,
                result: Ok(saved.clone()),
            },
        );
        assert!(stale.effects.is_empty());
        assert_eq!(stale.session, saving.session);
        let mut changed = saved.clone();
        changed["thinking"]["choice"] = json!("byo");
        for response in [json!({}), changed] {
            let stopped = reduce(
                &saving.session,
                &Event::ActionCompleted {
                    generation,
                    result: Ok(response),
                },
            );
            assert!(stopped.effects.is_empty());
            assert!(stopped.session.continue_after_save.is_none());
        }
        let failed = reduce(
            &saving.session,
            &Event::ActionCompleted {
                generation,
                result: Err(ActionErrorPayload {
                    reason_code: None,
                    message: "save refused".into(),
                    is_transport: true,
                }),
            },
        );
        assert!(failed.session.continue_after_save.is_none());
        assert_eq!(failed.effects, vec![Effect::FetchState]);
        let reconciled = reduce(
            &failed.session,
            &Event::StateLoaded {
                generation: failed.session.inflight.unwrap(),
                result: Ok(saved.clone()),
            },
        );
        assert!(reconciled.effects.is_empty());
        let complete = reduce(
            &saving.session,
            &Event::ActionCompleted {
                generation,
                result: Ok(saved),
            },
        );
        assert_eq!(
            complete.effects,
            vec![Effect::RecordPending {
                generation: complete.session.inflight.unwrap(),
                timezone: None,
            }]
        );
    }

    #[test]
    fn empty_or_stripped_glyphs_never_admit_a_mark() {
        for svg in [
            "<path/>",
            "<path d=\"\"/>",
            "<path d=\"M0 0\"/>",
            "<circle/>",
            "<rect width=\"0\" height=\"1\"/>",
            "<script>draw</script>",
            "<path",
        ] {
            let mut render = sample_render();
            render["icon1"]["svg"] = json!(svg);
            assert!(admit_mark_render(&render).is_err(), "{svg}");
        }
        assert!(admit_mark_render(&sample_render()).is_ok());
    }
    #[test]
    fn matching_saved_local_continues_without_rechecking_current_capability() {
        for capability in ["blocked", "unknown"] {
            let mut state = setup_state();
            state["mark"]["state"] = json!("locked");
            state["thinking"]["saved"] = json!(true);
            state["thinking"]["choice"] = json!("local");
            state["thinking"]["capability"] = json!(capability);
            state["remaining_required_steps"] = json!([]);
            state["downloads"]["thinking"]["state"] = json!("error");
            state["downloads"]["thinking"]["error"] =
                json!({"code": "download_failed", "detail": "download failed"});
            let mut session = Session::default();
            apply_snapshot(&mut session, &mut vec![], &state);
            assert!(session.authoritative);
            let continued = reduce(
                &session,
                &Event::ContinueThinking {
                    choice: "local".into(),
                    timezone: None,
                },
            );
            assert_eq!(
                continued.effects,
                vec![Effect::RecordPending {
                    generation: continued.session.inflight.unwrap(),
                    timezone: None,
                }]
            );
            assert!(continued.session.continue_after_save.is_none());
            let mut other_saved = state;
            other_saved["thinking"]["choice"] = json!("byo");
            apply_snapshot(&mut session, &mut vec![], &other_saved);
            let refused = reduce(
                &session,
                &Event::ContinueThinking {
                    choice: "local".into(),
                    timezone: None,
                },
            );
            assert!(refused.effects.is_empty());
        }
    }
    #[test]
    fn unchanged_catalog_geometry_is_admitted_including_implicit_lines() {
        let catalog: std::collections::BTreeMap<String, String> = serde_json::from_str(
            include_str!("../../solstone-core-sol-link/assets/mark_assets/glyphs.json"),
        )
        .unwrap();
        assert!(catalog.contains_key("drum"));
        for (name, svg) in catalog {
            assert!(svg_has_geometry(&svg), "catalog glyph rejected: {name}");
        }
        for path in ["m2 2 8 8", "m22 2-8 8", "M20 4 8.12 15.88"] {
            assert!(path_has_geometry(path), "{path}");
        }
        assert!(!path_has_geometry("M0 0"));
        assert!(!path_has_geometry("M0 0L"));
    }

    #[test]
    fn pending_continuation_before_finalize_and_receipt_browser() {
        let mut state = setup_state();
        state["mark"]["state"] = json!("locked");
        state["mark"]["render"] = sample_render();
        state["thinking"]["saved"] = json!(true);
        state["thinking"]["choice"] = json!("local");
        state["remaining_required_steps"] = json!([]);
        let mut session = Session::default();
        apply_snapshot(&mut session, &mut vec![], &state);
        assert_eq!(session.phase, Phase::Thinking);
        assert!(session.authoritative);

        let finalize = reduce(
            &session,
            &Event::FinalizeClicked {
                timezone: Some("America/Denver".into()),
            },
        );
        assert_eq!(finalize.browser_effect(), BrowserEffect::None);
        let generation = finalize.session.inflight.unwrap();
        assert_eq!(
            finalize.effects,
            vec![Effect::RecordPending {
                generation,
                timezone: Some("America/Denver".into()),
            }]
        );

        let pending_err = reduce(
            &finalize.session,
            &Event::PendingRecorded {
                generation,
                timezone: Some("America/Denver".into()),
                result: Err("disk full".into()),
            },
        );
        assert_eq!(pending_err.session.phase, Phase::Thinking);
        assert!(pending_err.effects.is_empty());
        assert!(pending_err.session.inflight.is_none());
        assert!(pending_err.session.error.is_some());

        let pending_ok = reduce(
            &finalize.session,
            &Event::PendingRecorded {
                generation,
                timezone: Some("America/Denver".into()),
                result: Ok(()),
            },
        );
        assert_eq!(pending_ok.browser_effect(), BrowserEffect::None);
        assert_eq!(
            pending_ok.effects,
            vec![Effect::PostAction(SetupAction::Finalize {
                timezone: Some("America/Denver".into()),
            })]
        );

        let mut boot_session = Session::default();
        let mut boot_effects = Vec::new();
        let mut uninstalled_state = setup_state();
        uninstalled_state["task_installed"] = json!(false);
        apply_snapshot(&mut boot_session, &mut boot_effects, &uninstalled_state);
        let boot_step = Step {
            session: boot_session,
            effects: boot_effects,
        };
        assert_eq!(boot_step.browser_effect(), BrowserEffect::None);
        assert!(
            !boot_step
                .effects
                .iter()
                .any(|e| matches!(e, Effect::ContinueInSolstone { .. }))
        );

        let mut home_session = Session::default();
        let mut home_effects = Vec::new();
        let mut done_state = setup_state();
        done_state["done"] = json!(true);
        done_state["remaining_required_steps"] = json!([]);
        apply_snapshot(&mut home_session, &mut home_effects, &done_state);
        let home_step = Step {
            session: home_session.clone(),
            effects: home_effects.clone(),
        };
        assert_eq!(home_step.session.phase, Phase::Home);
        assert_eq!(home_step.browser_effect(), BrowserEffect::None);
        assert!(home_effects.contains(&Effect::ObserveReceipt {
            generation: home_session.generation,
        }));

        let shown = reduce(&home_session, &Event::HomeShown);
        assert_eq!(
            shown.effects,
            vec![Effect::ObserveReceipt {
                generation: home_session.generation,
            }]
        );

        let cont = reduce(&home_session, &Event::ContinueInSolstone);
        let cont_gen = cont.session.inflight.unwrap();
        assert_eq!(
            cont.effects,
            vec![Effect::ContinueInSolstone {
                generation: cont_gen,
            }]
        );

        let stale = reduce(
            &cont.session,
            &Event::ContinueFinished {
                generation: cont_gen + 999,
                outcome: ContinueOutcome::Published,
            },
        );
        assert_eq!(stale.session.inflight, Some(cont_gen));
        assert!(!stale.session.continuation_pending);

        let published = reduce(
            &cont.session,
            &Event::ContinueFinished {
                generation: cont_gen,
                outcome: ContinueOutcome::Published,
            },
        );
        assert_eq!(published.browser_effect(), BrowserEffect::None);
        assert!(published.session.continuation_pending);
        assert!(published.session.inflight.is_none());

        let observed_false = reduce(
            &published.session,
            &Event::ContinuationObserved {
                generation: published.session.generation,
                pending: false,
            },
        );
        assert!(!observed_false.session.continuation_pending);
        let v = view(&observed_false.session);
        assert_eq!(v["phase"], "home");
        assert!(v["step"].is_null());
        assert_eq!(v["continuation_pending"], false);

        let cont_again = reduce(&observed_false.session, &Event::ContinueInSolstone);
        assert_eq!(
            cont_again.effects,
            vec![Effect::ContinueInSolstone {
                generation: cont_again.session.inflight.unwrap(),
            }]
        );

        let dl_home = reduce(&home_session, &Event::DownloadSolstoneClicked);
        assert_eq!(dl_home.effects, vec![Effect::OpenSolstoneDownload]);
        assert_eq!(dl_home.browser_effect(), BrowserEffect::None);

        let dl_off_home = reduce(&session, &Event::DownloadSolstoneClicked);
        assert!(dl_off_home.effects.is_empty());
    }
}

#[cfg(test)]
mod release_activation_tests {
    use super::*;
    #[test]
    fn browser_first_setup_preserves_published_arguments() {
        let args = legacy_setup_argv(Path::new("C:/owner journal"));
        assert_eq!(
            args,
            [
                "setup",
                "--jsonl",
                "--yes",
                "--accept-existing-journal",
                "--journal",
                "C:/owner journal"
            ]
        );
        assert!(!args.iter().any(|arg| arg.starts_with("--initial-")
            || arg == "--skip-models"
            || arg == "--skip-brain"));
    }
}
