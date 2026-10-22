// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The window. One `journal` window in the taskbar: closing it keeps it
//! there, as closing the Mac app's window keeps it in the dock, and quitting
//! the app stops the journal, as quitting the Mac app does.
//!
//! The window is a web view over pages built into this program. The page asks
//! for everything through one message channel; it cannot reach the network
//! or the disk itself, and it can only call the journal routes listed here.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::convey::{Convey, InitProbe, Refusal};
use crate::prefs::{self, CheckInterval};
use crate::shell;
use crate::status::{ServiceStatus, run_display};
use crate::update::Updater;
use crate::webview::{Page, Pages};
use crate::window::{Proxy, UserEvent, Window, WindowEvent};
use crate::{ico, journal};

const ORIGIN: &str = "https://journal.localhost";
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; font-src 'self'; connect-src 'none'";

/// How this run of the app began.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Launch {
    /// The owner opened the app: like opening the Mac app, it starts the
    /// journal.
    Plain,
    /// The owner signed in: the journal's own task starts it.
    SignIn,
    /// The installer reopened the app after an update: the update's own step
    /// puts the journal back only if the owner had it running.
    AfterUpdate,
}

/// The pages built into the app, and the journal's own type and font.
fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path {
        "/" | "/index.html" => (include_bytes!("../ui/index.html"), "text/html"),
        "/app.css" => (include_bytes!("../ui/app.css"), "text/css"),
        "/app.js" => (include_bytes!("../ui/app.js"), "text/javascript"),
        "/mark.js" => (include_bytes!("../ui/mark.js"), "text/javascript"),
        // The journal's own QR code maker, the one its pairing page draws with.
        "/pairing-qr.js" => (
            include_bytes!("../../solstone-core-convey-shell/assets/static/pairing-qr.js"),
            "text/javascript",
        ),
        // The sun arc behind setup: the journal's own engine and the mark it draws, at the
        // path the engine asks for.
        "/sunarc.js" => (
            include_bytes!("../../solstone-core-convey-shell/assets/static/sunarc.js"),
            "text/javascript",
        ),
        "/static/sol-status/mark.svg" => (
            include_bytes!("../../solstone-core-convey-shell/assets/static/sol-status/mark.svg"),
            "image/svg+xml",
        ),
        "/tokens.css" => (
            include_bytes!("../../solstone-core-convey-shell/assets/static/tokens.css"),
            "text/css",
        ),
        "/tokens-dark.css" => (
            include_bytes!("../../solstone-core-convey-shell/assets/static/tokens-dark.css"),
            "text/css",
        ),
        "/Comfortaa-Variable.woff2" => (
            include_bytes!(
                "../../solstone-core-convey-shell/assets/static/Comfortaa-Variable.woff2"
            ),
            "font/woff2",
        ),
        "/lockup-solstone-horizontal.svg" => (
            include_bytes!("../ui/lockup-solstone-horizontal.svg"),
            "image/svg+xml",
        ),
        _ => return None,
    })
}

/// The only journal routes the page may call, with the methods each takes.
fn convey_route_allowed(method: &str, path: &str) -> bool {
    crate::setup_flow::convey_route_allowed(method, path)
}

/// The pages the app opens in the owner's browser, by name, never by URL.
/// The window finishes setup itself. The journal UI still opens in the browser
/// when the owner asks.
fn journal_page(name: &str) -> Option<&'static str> {
    Some(match name {
        "journal" => crate::setup_flow::APP_HOME_PATH,
        "init" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => "/init",
        "backup" => "/app/backup",
        "devices" => "/app/network",
        _ => return None,
    })
}

/// `http://127.0.0.1:<port>` and nothing more.
fn is_loopback_base(base: &str) -> bool {
    base.strip_prefix("http://127.0.0.1:")
        .is_some_and(|port| port.parse::<u16>().is_ok())
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn disk_usage(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // A link or junction is not part of the journal's own size.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                total += metadata.len();
            }
        }
    }
    total
}

struct Context {
    launch: Launch,
    proxy: Proxy,
    updater: Updater,
    session: std::rc::Rc<std::cell::RefCell<crate::setup_flow::Session>>,
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

fn journal_port() -> Option<u16> {
    journal::status().ok().and_then(|status| status.port)
}

fn dispatch_effect(context: &Context, effect: &crate::setup_flow::Effect, port: Option<u16>) {
    if !crate::setup_flow::SETUP_SEQUENCE_ENABLED {
        return;
    }
    match effect {
        crate::setup_flow::Effect::FetchCopy => {
            let proxy = context.proxy.clone();
            let current_gen = context.session.borrow().inflight.unwrap_or(0);
            std::thread::spawn(move || {
                let result = crate::journal::setup_copy();
                proxy.send_event(UserEvent::Flow(crate::setup_flow::Event::CopyLoaded {
                    generation: current_gen,
                    result,
                }));
            });
        }
        crate::setup_flow::Effect::FetchState => {
            let Some(port) = port else {
                return;
            };
            let proxy = context.proxy.clone();
            let current_gen = context.session.borrow().inflight.unwrap_or(0);
            std::thread::spawn(move || {
                let result = Convey::new(Some(port)).setup_state();
                proxy.send_event(UserEvent::Flow(crate::setup_flow::Event::StateLoaded {
                    generation: current_gen,
                    result,
                }));
            });
        }
        crate::setup_flow::Effect::PostAction(action) => {
            let Some(port) = port else {
                return;
            };
            let proxy = context.proxy.clone();
            let current_gen = context.session.borrow().inflight.unwrap_or(0);
            let action = action.clone();
            std::thread::spawn(move || {
                let result = Convey::new(Some(port)).setup_action(&action);
                proxy.send_event(UserEvent::Flow(crate::setup_flow::Event::ActionCompleted {
                    generation: current_gen,
                    result,
                }));
            });
        }
        crate::setup_flow::Effect::RunSetup {
            journal,
            task_installed,
            start_at_sign_in,
        } => {
            let proxy = context.proxy.clone();
            let progress_proxy = proxy.clone();
            let inflight_gen = context.session.borrow().inflight.unwrap_or(0);
            let args = crate::setup_flow::setup_argv(journal, *task_installed, *start_at_sign_in);
            std::thread::spawn(move || {
                let result = journal::setup(&args, move |progress| {
                    let message = json!({"type": "setup", "progress": progress});
                    progress_proxy.send_event(UserEvent::Script(format!(
                        "window.journalApp.receive({message});"
                    )));
                });
                proxy.send_event(UserEvent::Flow(crate::setup_flow::Event::SetupFinished {
                    generation: inflight_gen,
                    result,
                }));
            });
        }
        crate::setup_flow::Effect::SetSignInLaunch(on) => {
            if let Err(message) = shell::set_sign_in_launch(*on) {
                context.push(json!({"type": "sign-in-launch", "error": message}));
            }
        }
        crate::setup_flow::Effect::CheckModels { generation } => {
            let proxy = context.proxy.clone();
            let current_gen = *generation;
            std::thread::spawn(move || {
                let result = journal::models_ready();
                proxy.send_event(UserEvent::Flow(
                    crate::setup_flow::Event::ModelsCheckFinished {
                        generation: current_gen,
                        result,
                    },
                ));
            });
        }
        crate::setup_flow::Effect::InstallModels { generation } => {
            let proxy = context.proxy.clone();
            let current_gen = *generation;
            std::thread::spawn(move || {
                let result = journal::install_models();
                proxy.send_event(UserEvent::Flow(
                    crate::setup_flow::Event::ModelsInstallFinished {
                        generation: current_gen,
                        result,
                    },
                ));
            });
        }
        crate::setup_flow::Effect::RecordPending {
            generation,
            timezone,
        } => {
            let proxy = context.proxy.clone();
            let expected_gen = *generation;
            let tz = timezone.clone();
            std::thread::spawn(move || {
                let local_appdata = match std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                    Some(p) => p,
                    None => {
                        proxy.send_event(UserEvent::Flow(
                            crate::setup_flow::Event::PendingRecorded {
                                generation: expected_gen,
                                timezone: tz,
                                result: Err("could not save this step. try again.".to_owned()),
                            },
                        ));
                        return;
                    }
                };
                let contract = crate::setup_handoff::get_contract();
                let lock_path = local_appdata.join("Solstone").join(&contract.files.lock);
                let state_dir = crate::prefs::state_dir();
                let mut clock = crate::setup_handoff::WallClock;
                let outcome: Result<(), String> = (|| {
                    let _lock = crate::setup_handoff::HandoffLock::acquire(
                        &lock_path,
                        contract.limits.lock_acquire_ms,
                        &mut clock,
                    )
                    .map_err(|_| "could not save this step. try again.".to_owned())?;
                    let id = crate::setup_handoff::generate_uuid();
                    crate::setup_handoff::write_pending(&state_dir, &id)
                        .map_err(|_| "could not save this step. try again.".to_owned())?;
                    Ok(())
                })();
                proxy.send_event(UserEvent::Flow(crate::setup_flow::Event::PendingRecorded {
                    generation: expected_gen,
                    timezone: tz,
                    result: outcome,
                }));
            });
        }
        crate::setup_flow::Effect::ContinueInSolstone { generation } => {
            let proxy = context.proxy.clone();
            let expected_gen = *generation;
            let gen_arc = context.generation.clone();
            std::thread::spawn(move || {
                let local_appdata = match std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                    Some(p) => p,
                    None => {
                        proxy.send_event(UserEvent::Flow(
                            crate::setup_flow::Event::ContinueFinished {
                                generation: expected_gen,
                                outcome: crate::setup_flow::ContinueOutcome::Failed,
                            },
                        ));
                        return;
                    }
                };
                let Some(port) = port else {
                    proxy.send_event(UserEvent::Flow(
                        crate::setup_flow::Event::ContinueFinished {
                            generation: expected_gen,
                            outcome: crate::setup_flow::ContinueOutcome::Failed,
                        },
                    ));
                    return;
                };
                let paths = crate::setup_handoff::HandoffPaths {
                    local_appdata,
                    state_dir: crate::prefs::state_dir(),
                };
                let convey = Convey::new(Some(port));
                let collaborators = crate::setup_handoff::HandoffCollaborators {
                    paths,
                    endpoints: &convey,
                    prepare: |path| {
                        crate::setup_receiver::PreparedReceiver::open(
                            path,
                            &crate::setup_handoff::get_contract(),
                        )
                    },
                    launch: |receiver: &crate::setup_receiver::PreparedReceiver, _, flag| {
                        receiver.launch(flag)
                    },
                    still_current: move || {
                        gen_arc.load(std::sync::atomic::Ordering::SeqCst) == expected_gen
                    },
                    clock: crate::setup_handoff::WallClock,
                    new_id: crate::setup_handoff::generate_uuid,
                };
                let outcome = crate::setup_handoff::continue_in_solstone(collaborators);
                proxy.send_event(UserEvent::Flow(
                    crate::setup_flow::Event::ContinueFinished {
                        generation: expected_gen,
                        outcome,
                    },
                ));
            });
        }
        crate::setup_flow::Effect::ObserveReceipt { generation } => {
            let proxy = context.proxy.clone();
            let expected_gen = *generation;
            std::thread::spawn(move || {
                let local_appdata = match std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                    Some(p) => p,
                    None => return,
                };
                let paths = crate::setup_handoff::HandoffPaths {
                    local_appdata,
                    state_dir: crate::prefs::state_dir(),
                };
                let mut clock = crate::setup_handoff::WallClock;
                if let Ok(pending) = crate::setup_handoff::observe_receipt(&paths, &mut clock) {
                    proxy.send_event(UserEvent::Flow(
                        crate::setup_flow::Event::ContinuationObserved {
                            generation: expected_gen,
                            pending,
                        },
                    ));
                }
            });
        }
        crate::setup_flow::Effect::OpenSolstoneDownload => {
            let _ = shell::open("https://solstone.app/download/windows");
        }
        crate::setup_flow::Effect::Browser(crate::setup_flow::BrowserEffect::OpenHome) => {
            let Some(port) = port else {
                return;
            };
            let _ = shell::open(&crate::setup_flow::home_url(port));
        }
        crate::setup_flow::Effect::Browser(
            crate::setup_flow::BrowserEffect::Rejected | crate::setup_flow::BrowserEffect::None,
        ) => {}
    }
}

fn run_turn(context: &Context, step: crate::setup_flow::Step, reply: impl FnOnce(Value)) {
    let mut step = step;
    let bound =
        crate::setup_flow::withhold_remote(&mut step.session, &mut step.effects, journal_port());
    let rendered = crate::setup_flow::view(&step.session);
    *context.session.borrow_mut() = step.session;
    context.generation.store(
        context.session.borrow().generation,
        std::sync::atomic::Ordering::SeqCst,
    );
    let port = bound.ok();
    crate::setup_flow::execute_turn(rendered, &step.effects, reply, |effect| {
        dispatch_effect(context, effect, port);
    });
}

impl Context {
    fn reply(&self, id: &Value, result: Result<Value, String>) {
        let message = match result {
            Ok(value) => json!({"type": "reply", "id": id, "ok": true, "value": value}),
            Err(error) => json!({"type": "reply", "id": id, "ok": false, "error": error}),
        };
        self.push(message);
    }

    fn push(&self, message: Value) {
        self.proxy.send_event(UserEvent::Script(format!(
            "window.journalApp.receive({message});"
        )));
    }

    /// Run `work` off the window's thread and answer the page with its result.
    fn spawn(&self, id: Value, work: impl FnOnce() -> Result<Value, String> + Send + 'static) {
        self.spawn_convey(id, move || work().map_err(Refusal::from));
    }

    /// As `spawn`, for a journal route: a refusal reaches the page with the
    /// journal's status and reason code beside its words.
    fn spawn_convey(
        &self,
        id: Value,
        work: impl FnOnce() -> Result<Value, Refusal> + Send + 'static,
    ) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let message = match work() {
                Ok(value) => json!({"type": "reply", "id": id, "ok": true, "value": value}),
                Err(refusal) => json!({
                    "type": "reply",
                    "id": id,
                    "ok": false,
                    "error": refusal.message,
                    "status": refusal.status,
                    "code": refusal.reason_code,
                }),
            };
            proxy.send_event(UserEvent::Script(format!(
                "window.journalApp.receive({message});"
            )));
        });
    }
}

fn read_status() -> Result<Value, String> {
    let service = journal::status()?;
    let (answering, version) = if service.is_set_up() {
        Convey::new(service.port).answer()
    } else {
        (false, None)
    };
    Ok(status_value(&service, answering, version))
}

fn status_value(service: &ServiceStatus, answering: bool, version: Option<String>) -> Value {
    json!({
        "service": service,
        "set_up": service.is_set_up(),
        "answering": answering,
        "runtime_version": version,
        "about": solstone_core_about::host_about(version.as_deref().unwrap_or(env!("CARGO_PKG_VERSION"))).about,
        "display": run_display(service, answering),
        "base": Convey::new(service.port).base(),
    })
}

fn handle(context: &Context, window: &Window, message: &str) {
    let Ok(request) = serde_json::from_str::<Value>(message) else {
        return;
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let args = request.get("args").cloned().unwrap_or(Value::Null);
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let command = request
        .get("cmd")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match command {
        "init" => context.reply(
            &id,
            Ok(json!({
                "launch": match context.launch {
                    Launch::Plain => "plain",
                    Launch::SignIn => "sign-in",
                    Launch::AfterUpdate => "after-update",
                },
                "app_version": env!("CARGO_PKG_VERSION"),
                "about": solstone_core_about::host_about(env!("CARGO_PKG_VERSION")).about,
                "default_location": home_dir().join("journal"),
                // The mark the owner locked in, as the app last drew it, so a
                // stopped journal still shows its own mark.
                "mark": prefs::load().icon_mark,
                "setup_sequence_enabled": crate::setup_flow::SETUP_SEQUENCE_ENABLED,
                "flow": crate::setup_flow::view(&context.session.borrow()),
            })),
        ),
        "status" => context.spawn(id, read_status),
        "ping" => {
            let port = args
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            context.spawn(id, move || {
                let (answering, version) = Convey::new(port).answer();
                Ok(json!({"answering": answering, "runtime_version": version}))
            });
        }
        "start" | "stop" | "restart" => {
            let command = command.to_owned();
            context.spawn(id, move || {
                match command.as_str() {
                    "start" => journal::start(),
                    "stop" => journal::stop(),
                    _ => journal::restart(),
                }?;
                read_status()
            });
        }
        "signIn" => {
            let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
            context.spawn(id, move || {
                journal::set_starts_at_sign_in(on)?;
                shell::set_sign_in_launch(on)?;
                read_status()
            });
        }
        "syncSignInLaunch" => {
            let Some(on) = args.get("on").and_then(Value::as_bool) else {
                context.reply(&id, Err("missing on parameter".to_owned()));
                return;
            };
            context.reply(&id, shell::set_sign_in_launch(on).map(|()| Value::Null));
        }
        "installModels" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => {
            context.spawn(id, || journal::install_models().map(|()| Value::Null));
        }
        "modelsReady" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => {
            context.spawn(id, || Ok(json!(journal::models_ready())));
        }
        "installModels" => {
            let session = context.session.borrow().clone();
            let step = crate::setup_flow::reduce(
                &session,
                &crate::setup_flow::Event::RetryModelsInstallClicked,
            );
            run_turn(context, step, |view| context.reply(&id, Ok(view)));
        }
        "modelsReady" => {
            let session = context.session.borrow().clone();
            let step =
                crate::setup_flow::reduce(&session, &crate::setup_flow::Event::RequestModelsCheck);
            run_turn(context, step, |_view| context.reply(&id, Ok(Value::Null)));
        }
        "pickFolder" => {
            let start = text("start").map(PathBuf::from);
            let picked = shell::pick_folder(window.hwnd(), start.as_deref());
            context.reply(
                &id,
                picked.map(|path| {
                    path.map_or(Value::Null, |path| {
                        json!({"path": path, "holds_journal": journal::holds_a_journal(&path)})
                    })
                }),
            );
        }
        "holdsJournal" => {
            let path = PathBuf::from(text("path").unwrap_or_default());
            context.reply(&id, Ok(json!(journal::holds_a_journal(&path))));
        }
        "setup" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => {
            let Some(path) = text("path").filter(|path| !path.trim().is_empty()) else {
                context.reply(&id, Err("choose where your journal lives".to_owned()));
                return;
            };
            let proxy = context.proxy.clone();
            context.spawn(id, move || {
                let args = crate::setup_flow::legacy_setup_argv(Path::new(&path));
                let models_missing = journal::setup(&args, |progress| {
                    let message = json!({"type": "setup", "progress": progress});
                    proxy.send_event(UserEvent::Script(format!(
                        "window.journalApp.receive({message});"
                    )));
                })?;
                Ok(json!({"status": read_status()?, "models_missing": models_missing}))
            });
        }
        "initProbe" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => {
            context.spawn(id, move || {
                let port =
                    journal_port().ok_or_else(|| "the journal isn't answering".to_owned())?;
                Convey::new(Some(port)).init_probe().map(|probe| {
                    json!(match probe {
                        InitProbe::Complete => "complete",
                        InitProbe::Incomplete => "incomplete",
                    })
                })
            });
        }
        "convey" => {
            let method = text("method").unwrap_or_else(|| "GET".to_owned());
            let path = text("path").unwrap_or_default();
            if !crate::setup_flow::SETUP_SEQUENCE_ENABLED && path.starts_with("/init/") {
                context.reply(
                    &id,
                    Err("native setup actions are unavailable in this build".into()),
                );
                return;
            }
            let renderer_port = args
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            let target_port = match crate::setup_flow::convey_dispatch(
                &method,
                &path,
                renderer_port,
                journal_port(),
            ) {
                crate::setup_flow::ConveyDispatch::Rejected => {
                    context.reply(
                        &id,
                        Err(format!(
                            "{method} {path} isn't a journal route the app uses"
                        )),
                    );
                    return;
                }
                crate::setup_flow::ConveyDispatch::Unanswered => {
                    context.reply(&id, Err("the journal isn't answering".to_owned()));
                    return;
                }
                crate::setup_flow::ConveyDispatch::Use(port) => port,
            };
            let body = args.get("body").cloned();
            context.spawn_convey(id, move || {
                let convey = Convey::new(target_port);
                match method.as_str() {
                    "GET" => convey.get(&path),
                    "PUT" => convey.put(&path, body.as_ref().unwrap_or(&Value::Null)),
                    _ => convey.post(&path, body.as_ref()),
                }
            });
        }
        "pairingStatus" => {
            let port = args
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            let nonce = text("nonce").unwrap_or_default();
            context.spawn_convey(id, move || Convey::new(port).pairing_status(&nonce));
        }
        "diskUsage" => {
            let path = PathBuf::from(text("path").unwrap_or_default());
            context.spawn(id, move || Ok(json!(disk_usage(&path))));
        }
        "open" => {
            let base = text("base")
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", crate::convey::DEFAULT_PORT));
            let target = text("target").unwrap_or_default();
            let result = if target == "folder" {
                let path = text("path").unwrap_or_default();
                if Path::new(&path).is_dir() {
                    shell::open(&path)
                } else {
                    Err("that folder isn't there".to_owned())
                }
            } else if !crate::setup_flow::SETUP_SEQUENCE_ENABLED
                && matches!(target.as_str(), "journal" | "init")
            {
                journal_port()
                    .ok_or_else(|| "the journal isn't answering".to_owned())
                    .and_then(|port| {
                        let path = if target == "init" {
                            "/init"
                        } else {
                            crate::setup_flow::APP_HOME_PATH
                        };
                        shell::open(&format!("http://127.0.0.1:{port}{path}"))
                    })
            } else if target == "journal" {
                let session = context.session.borrow().clone();
                let mut step = crate::setup_flow::reduce(
                    &session,
                    &crate::setup_flow::Event::OpenJournalClicked,
                );
                let wanted = step.browser_effect();
                let bound = crate::setup_flow::withhold_remote(
                    &mut step.session,
                    &mut step.effects,
                    journal_port(),
                );
                *context.session.borrow_mut() = step.session;
                match wanted {
                    crate::setup_flow::BrowserEffect::OpenHome => match bound {
                        Ok(port) => shell::open(&crate::setup_flow::home_url(port)),
                        Err(()) => Err(context
                            .session
                            .borrow()
                            .error
                            .clone()
                            .unwrap_or_else(|| "the journal isn't answering".to_owned())),
                    },
                    crate::setup_flow::BrowserEffect::Rejected => {
                        Err("unexpected redirect destination".to_owned())
                    }
                    crate::setup_flow::BrowserEffect::None => Ok(()),
                }
            } else if let Some(page) = journal_page(&target) {
                // Only the journal's own loopback address opens from here.
                if is_loopback_base(&base) {
                    shell::open(&format!("{base}{page}"))
                } else {
                    Err("that isn't your journal's address".to_owned())
                }
            } else {
                Err("that page isn't one the app opens".to_owned())
            };
            context.reply(&id, result.map(|()| Value::Null));
        }
        "flow" if !crate::setup_flow::SETUP_SEQUENCE_ENABLED => {
            context.reply(
                &id,
                Err("setup sequence is unavailable in this build".to_owned()),
            );
        }
        "flow" => {
            let intent = text("intent").unwrap_or_default();
            if matches!(intent.as_str(), "start-setup" | "startSetup") {
                if let Some(path) = text("path").filter(|path| !path.trim().is_empty()) {
                    context.session.borrow_mut().journal_path = PathBuf::from(path);
                }
            }
            let event = match crate::setup_flow::event_from_intent(&intent, &args) {
                Ok(event) => event,
                Err(error) => {
                    context.reply(&id, Err(error));
                    return;
                }
            };
            let session = context.session.borrow().clone();
            let step = crate::setup_flow::reduce(&session, &event);
            run_turn(context, step, |view| context.reply(&id, Ok(view)));
        }
        "adminTerminal" => {
            context.reply(
                &id,
                shell::open_admin_terminal(&journal::bin_dir(), &home_dir()).map(|()| Value::Null),
            );
        }
        "setIcon" => context.reply(&id, set_icon(window, &args)),
        "update" => {
            let updater = context.updater.clone();
            match text("action").as_deref() {
                Some("check") => context.spawn(id, move || {
                    updater.check();
                    Ok(updater.view())
                }),
                Some("download") => context.spawn(id, move || {
                    updater.download();
                    Ok(updater.view())
                }),
                Some("install") => context.spawn(id, move || {
                    updater.install();
                    Ok(updater.view())
                }),
                Some("prefs") => {
                    let interval = match text("interval").as_deref() {
                        Some("day") => CheckInterval::Day,
                        Some("month") => CheckInterval::Month,
                        _ => CheckInterval::Week,
                    };
                    updater.set_prefs(
                        args.get("auto_check")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        args.get("auto_download")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        interval,
                    );
                    context.reply(&id, Ok(updater.view()));
                }
                _ => context.reply(&id, Ok(updater.view())),
            }
        }
        "minimize" => {
            window.minimize();
            context.reply(&id, Ok(Value::Null));
        }
        "quit" => {
            // Quitting stops the journal, as it does on the Mac. The window
            // stays up saying so until the stop has finished.
            let stop = args.get("stop").and_then(Value::as_bool).unwrap_or(true);
            if stop {
                let proxy = context.proxy.clone();
                context.spawn(id, move || {
                    let result = journal::stop();
                    proxy.send_event(UserEvent::Exit);
                    result.map(|()| Value::Null)
                });
            } else {
                context.proxy.send_event(UserEvent::Exit);
            }
        }
        _ => context.reply(&id, Err(format!("unknown request {command}"))),
    }
}

/// Draw the journal's mark as the app's icon: on the window and taskbar
/// button now, and on the Start-menu entry, as the Mac app does once the
/// owner locks the mark in.
fn set_icon(window: &Window, args: &Value) -> Result<Value, String> {
    let images = args
        .get("images")
        .and_then(Value::as_array)
        .ok_or("no icon images")?
        .iter()
        .map(|image| {
            let side = image.get(0).and_then(Value::as_u64).ok_or("no icon size")? as u32;
            let bytes = image
                .get(1)
                .and_then(Value::as_array)
                .ok_or("no icon image")?
                .iter()
                .map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                .collect::<Option<Vec<u8>>>()
                .ok_or("an icon image isn't bytes")?;
            Ok((side, bytes))
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let icon = ico::pack_png_icon(&images)?;
    let path = prefs::mark_icon_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("ico.tmp");
    std::fs::write(&temporary, icon).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    shell::set_window_icon(window.hwnd(), Some(&path));
    let shortcut = shell::set_shortcut_icon(Some(&path))?;
    let mut saved = prefs::load();
    saved.icon_mark = args.get("mark").cloned();
    prefs::save(&saved);
    Ok(json!({"shortcut": shortcut}))
}

pub fn run(launch: Launch, update_feed: Option<String>) {
    let Some(instance) = shell::SingleInstance::acquire() else {
        return;
    };
    // A sign-in start waits in the taskbar; the owner opens it from there.
    let window = Window::new("journal", launch == Launch::SignIn);
    let proxy = window.proxy();
    {
        let proxy = proxy.clone();
        instance.on_show(move || proxy.send_event(UserEvent::Show));
    }

    // The mark the owner locked in, if the app has drawn it before;
    // otherwise the program's own icon.
    let mark_icon = prefs::mark_icon_path();
    let have_mark_icon = mark_icon.is_file() && prefs::load().icon_mark.is_some();
    shell::set_window_icon(window.hwnd(), have_mark_icon.then_some(mark_icon.as_path()));
    if have_mark_icon {
        // An update or a reinstall can put the Start-menu entry back on the
        // program's own icon; put the mark back, as the Mac app does at launch.
        let _ = shell::set_shortcut_icon(Some(&mark_icon));
    }

    let updater = {
        let proxy = proxy.clone();
        Updater::new(update_feed.as_deref(), move |view| {
            let message = json!({"type": "update", "view": view});
            proxy.send_event(UserEvent::Script(format!(
                "window.journalApp.receive({message});"
            )));
        })
    };
    updater.run_schedule();

    let ipc_proxy = proxy.clone();
    let page = match Page::new(
        window.hwnd(),
        &prefs::webview_dir(),
        Pages {
            origin: ORIGIN,
            start: "/index.html",
            asset,
            content_security_policy: CONTENT_SECURITY_POLICY,
        },
        move |message| ipc_proxy.send_event(UserEvent::Ipc(message)),
        // A link the owner follows opens in their browser, and only the
        // journal's own loopback address.
        |url| {
            if url.starts_with("http://127.0.0.1:") {
                let _ = shell::open(&url);
            }
        },
    ) {
        Ok(page) => page,
        Err(reason) => {
            shell::alert("journal", &reason);
            return;
        }
    };

    let session = std::rc::Rc::new(std::cell::RefCell::new(
        crate::setup_flow::Session::default(),
    ));
    let context = Context {
        launch,
        proxy,
        updater,
        session: session.clone(),
        generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    let window = std::rc::Rc::new(window);
    let looped = window.clone();
    window.run(move |event| match event {
        WindowEvent::User(UserEvent::Ipc(message)) => handle(&context, &looped, &message),
        WindowEvent::User(UserEvent::Script(script)) => page.run_script(&script),
        WindowEvent::User(UserEvent::Flow(flow_event)) => {
            if !crate::setup_flow::SETUP_SEQUENCE_ENABLED {
                return;
            }
            let session = context.session.borrow().clone();
            let step = crate::setup_flow::reduce(&session, &flow_event);
            run_turn(&context, step, |view| {
                let message = json!({"type": "flow", "view": view});
                page.run_script(&format!("window.journalApp.receive({message});"));
            });
        }
        WindowEvent::User(UserEvent::Show) => {
            looped.bring_forward();
            page.fit(looped.hwnd(), false);
        }
        WindowEvent::Activated => {
            page.focus();
            page.run_script("window.journalApp.receive({type:'shown'});");
            let is_home = context.session.borrow().phase == crate::setup_flow::Phase::Home;
            if crate::setup_flow::SETUP_SEQUENCE_ENABLED && is_home {
                let session = context.session.borrow().clone();
                let step = crate::setup_flow::reduce(
                    &session,
                    &crate::setup_flow::Event::RequestModelsCheck,
                );
                run_turn(&context, step, |view| {
                    let message = json!({"type": "flow", "view": view});
                    page.run_script(&format!("window.journalApp.receive({message});"));
                });
                if context.session.borrow().phase == crate::setup_flow::Phase::Home {
                    let session = context.session.borrow().clone();
                    let step =
                        crate::setup_flow::reduce(&session, &crate::setup_flow::Event::HomeShown);
                    run_turn(&context, step, |view| {
                        let message = json!({"type": "flow", "view": view});
                        page.run_script(&format!("window.journalApp.receive({message});"));
                    });
                }
            }
        }
        WindowEvent::Resized { minimized } => page.fit(looped.hwnd(), minimized),
        WindowEvent::Moved => page.moved(),
        WindowEvent::User(UserEvent::Exit) => {
            page.close();
            looped.close();
        }
    });
}
