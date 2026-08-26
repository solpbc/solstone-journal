// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal config`.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use solstone_core_cli::{ConfigAction, ConfigCommand, ConfigJournalOptions};
use solstone_core_installation_identity::{
    OwnerBase, PlatformTag, SetupAdmissionRequest, admit_setup, journal_token_from_path,
    namespace_name, root_token_from_path,
};
use solstone_core_journal::{
    Source, detect_checkout_root, read_config_journal, resolve_journal_path,
};
use solstone_core_setup::{
    identity_evidence::gather_wrapper_artifact_evidence,
    manifest::{legacy_manifest_evidence, manifest_path},
    wrapper::{
        parse_wrapper, render_wrapper, wrapper_lock, wrapper_paths, write_wrappers_atomically,
    },
};

const MERGE_INSTRUCTIONS: &str = "journal config: --merge is temporarily unavailable.\nKeep both journal copies until archive merge support is migrated.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestedAction {
    Move,
    Switch,
    Merge,
    Force,
}
impl From<ConfigAction> for RequestedAction {
    fn from(v: ConfigAction) -> Self {
        match v {
            ConfigAction::Move => Self::Move,
            ConfigAction::Switch => Self::Switch,
            ConfigAction::Merge => Self::Merge,
            ConfigAction::Force => Self::Force,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Proceed,
    Move,
    Switch,
    Merge,
    Noop,
    Refuse,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalChange {
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub paths_equal: bool,
    pub current_active: bool,
    pub target_active: bool,
    pub current_exists: bool,
    pub target_exists: bool,
    pub target_parent_exists: bool,
    pub current_device: Option<u64>,
    pub target_parent_device: Option<u64>,
    pub same_filesystem: Option<bool>,
    pub service_installed: bool,
    pub service_running: bool,
    pub action: Option<RequestedAction>,
    pub yes: bool,
    pub dry_run: bool,
    pub sol_bin: PathBuf,
    pub service_bin: PathBuf,
    pub alias: PathBuf,
    pub home_dir: PathBuf,
    pub identity_root: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub action: Action,
    pub exit_code: u8,
    pub message: Option<String>,
    pub plan_only: bool,
}
fn decision(action: Action, exit_code: u8) -> Decision {
    Decision {
        action,
        exit_code,
        message: None,
        plan_only: false,
    }
}
fn state(active: bool) -> &'static str {
    if active { "active" } else { "not active" }
}
fn valid_flags(c: &JournalChange) -> &'static str {
    if c.current_active && !c.target_active {
        "--move, --switch"
    } else {
        "--switch, --merge, --force"
    }
}
fn refusal(c: &JournalChange) -> String {
    format!(
        "journal config: refused: current is {} and target is {}; valid flags: {}",
        state(c.current_active),
        state(c.target_active),
        valid_flags(c)
    )
}
fn missing_parent(c: &JournalChange) -> String {
    format!(
        "journal config: refused: move target parent does not exist: {}",
        c.target_path.parent().unwrap().display()
    )
}
fn missing_current(c: &JournalChange) -> String {
    format!(
        "journal config: refused: move source does not exist: {}",
        c.current_path.display()
    )
}
fn existing_target(c: &JournalChange) -> String {
    format!(
        "journal config: refused: move target already exists: {}",
        c.target_path.display()
    )
}
fn cross_filesystem(c: &JournalChange) -> String {
    format!(
        "journal config: refused: cannot move across filesystems (current device={}, target parent device={}); archive merge is temporarily unavailable, so keep both journal copies",
        c.current_device
            .map_or_else(|| "None".to_owned(), |v| v.to_string()),
        c.target_parent_device
            .map_or_else(|| "None".to_owned(), |v| v.to_string())
    )
}
pub(crate) fn decide(c: &JournalChange) -> Decision {
    if c.action == Some(RequestedAction::Merge) {
        return Decision {
            action: Action::Merge,
            exit_code: 1,
            message: Some(MERGE_INSTRUCTIONS.into()),
            plan_only: false,
        };
    }
    if c.paths_equal {
        return decision(Action::Noop, 0);
    }
    if c.action.is_none() {
        return if !c.current_active && !c.target_active {
            decision(Action::Proceed, 0)
        } else {
            Decision {
                action: Action::Refuse,
                exit_code: 1,
                message: Some(refusal(c)),
                plan_only: false,
            }
        };
    }
    if c.action == Some(RequestedAction::Force) {
        return decision(Action::Switch, 0);
    }
    if c.action == Some(RequestedAction::Move) {
        let message = if !c.target_parent_exists {
            Some(missing_parent(c))
        } else if !c.current_exists {
            Some(missing_current(c))
        } else if c.target_exists {
            Some(existing_target(c))
        } else if c.target_active {
            Some(format!(
                "journal config: refused: --move requires a not active target; current is {} and target is {}; valid flags: --switch, --merge, --force",
                state(c.current_active),
                state(c.target_active)
            ))
        } else if c.same_filesystem == Some(false) {
            Some(cross_filesystem(c))
        } else {
            None
        };
        if let Some(message) = message {
            return Decision {
                action: Action::Refuse,
                exit_code: 1,
                message: Some(message),
                plan_only: false,
            };
        }
        if c.dry_run {
            return Decision {
                action: Action::Move,
                exit_code: 0,
                message: None,
                plan_only: true,
            };
        }
        if !c.yes {
            return Decision {
                action: Action::Move,
                exit_code: 1,
                message: None,
                plan_only: true,
            };
        }
        return decision(Action::Move, 0);
    }
    if c.action == Some(RequestedAction::Switch) {
        if c.dry_run {
            return Decision {
                action: Action::Switch,
                exit_code: 0,
                message: None,
                plan_only: true,
            };
        }
        if !c.yes {
            return Decision {
                action: Action::Switch,
                exit_code: 1,
                message: None,
                plan_only: true,
            };
        }
        return decision(Action::Switch, 0);
    }
    Decision {
        action: Action::Refuse,
        exit_code: 1,
        message: Some(refusal(c)),
        plan_only: false,
    }
}
fn plan(c: &JournalChange, d: &Decision) -> String {
    let mut lines = vec![
        "journal config journal - plan summary".into(),
        "".into(),
        format!(
            "current: {} ({})",
            c.current_path.display(),
            state(c.current_active)
        ),
        format!(
            "target:  {} ({})",
            c.target_path.display(),
            state(c.target_active)
        ),
        format!(
            "action:  {}",
            match d.action {
                Action::Move => "move",
                Action::Switch => "switch",
                _ => "proceed",
            }
        ),
        service_summary(c, d).to_owned(),
    ];
    if d.action == Action::Move {
        // Intentionally preserve Python's truthy collapse of an unknown device.
        lines.push(if c.same_filesystem.unwrap_or(false) {
            "filesystem: same device".into()
        } else {
            "filesystem: different devices".into()
        });
    }
    if d.action == Action::Switch {
        lines.push(String::new());
        lines.push(
            "current journal is left intact. to re-adopt it later: journal config journal "
                .to_string()
                + &c.current_path.display().to_string()
                + " --switch --yes",
        );
    }
    lines.push(String::new());
    lines.push(if c.dry_run {
        "dry-run: yes; nothing will be changed".into()
    } else {
        "re-run with --yes to proceed".into()
    });
    lines.join("\n")
}
fn service_summary(c: &JournalChange, d: &Decision) -> &'static str {
    if d.action == Action::Move {
        if !c.service_installed {
            "service: not installed; will move and rewrite wrapper"
        } else if !c.service_running {
            "service: installed but not running; will move and rewrite wrapper"
        } else {
            "service: installed and running; will stop, move, rewrite wrapper, restart"
        }
    } else if !c.service_installed {
        "service: not installed; will rewrite wrapper"
    } else if !c.service_running {
        "service: installed but not running; will rewrite wrapper"
    } else {
        "service: installed and running; will rewrite wrapper, restart"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceCommand {
    Stop,
    Start,
    RestartIfInstalled,
}
pub(crate) enum ServiceCommandResult {
    Exited { code: i32 },
    ExecutableMissing { error: io::Error },
    LaunchError { error: io::Error },
}
pub(crate) trait ServiceCommandRunner {
    fn run(&self, executable: &Path, command: ServiceCommand) -> ServiceCommandResult;
}
struct RealServiceRunner;
impl ServiceCommandRunner for RealServiceRunner {
    fn run(&self, executable: &Path, command: ServiceCommand) -> ServiceCommandResult {
        let mut c = Command::new(executable);
        c.arg("service");
        match command {
            ServiceCommand::Stop => {
                c.arg("stop");
            }
            ServiceCommand::Start => {
                c.arg("start");
            }
            ServiceCommand::RestartIfInstalled => {
                c.args(["restart", "--if-installed"]);
            }
        };
        match c.status() {
            Ok(s) => ServiceCommandResult::Exited {
                code: s.code().unwrap_or(1),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                ServiceCommandResult::ExecutableMissing { error: e }
            }
            Err(e) => ServiceCommandResult::LaunchError { error: e },
        }
    }
}
fn wrapper_refusal(path: &Path) -> String {
    format!(
        "journal config: refused: {} is not a managed wrapper (run 'journal setup' from the solstone source checkout to install the wrapper first)",
        path.display()
    )
}
fn active(path: &Path) -> Result<bool, String> {
    if !path.is_dir() {
        return Ok(false);
    }
    let file = path.join("config/journal.json");
    let text = match fs::read_to_string(&file) {
        Ok(v) => v,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false);
        }
        Err(_) => return Err(corrupt(&file)),
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| corrupt(&file))?;
    let Some(object) = value.as_object() else {
        return Err(corrupt(&file));
    };
    Ok(object
        .get("setup")
        .and_then(serde_json::Value::as_object)
        .and_then(|s| s.get("completed_at"))
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|v| v > 0.0))
}
fn corrupt(path: &Path) -> String {
    format!(
        "I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.",
        path.display()
    )
}
fn installed() -> bool {
    if cfg!(target_os = "macos") {
        home()
            .join("Library/LaunchAgents/org.solpbc.solstone.plist")
            .exists()
    } else {
        home()
            .join(".config/systemd/user/solstone.service")
            .exists()
    }
}
fn running() -> bool {
    if !installed() {
        return false;
    }
    if cfg!(target_os = "macos") {
        Command::new("launchctl")
            .args([
                "print",
                &format!("gui/{}/org.solpbc.solstone", nix::unistd::Uid::effective()),
            ])
            .output()
            .is_ok_and(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout).contains("\n\tstate = running\n")
            })
    } else {
        Command::new("systemctl")
            .args(["--user", "is-active", "solstone"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
    }
}
fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
fn resolve_non_strict(path: &Path) -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    resolve_non_strict_from(path, &home(), &cwd)
}
fn resolve_non_strict_from(path: &Path, home: &Path, cwd: &Path) -> PathBuf {
    let mut components = path.components();
    let expanded = match components.next() {
        Some(std::path::Component::Normal(component)) if component == "~" => {
            home.join(components.as_path())
        }
        _ => path.to_path_buf(),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };

    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            std::path::Component::RootDir => resolved.push(Path::new("/")),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(component) => {
                let candidate = resolved.join(component);
                resolved = candidate.canonicalize().unwrap_or(candidate);
            }
        }
    }
    resolved
}
fn project_root() -> Option<PathBuf> {
    solstone_core::installation_context::project_root_from_current_executable()
}

fn identity_root_from_current_executable() -> Result<PathBuf, String> {
    solstone_core::installation_context::identity_root_from_current_executable()
        .map_err(|error| format!("journal config: {error}"))
}

fn is_source_checkout() -> bool {
    project_root().is_some_and(|root| root.join(".git").exists())
}
enum RewriteError {
    Refusal(String),
    Install(String),
}

fn legacy_manifest_for_rewrite(
    change: &JournalChange,
) -> solstone_core_installation_identity::LegacyManifestEvidence {
    // A switch keeps the current journal in place, whereas a move has already
    // renamed it to the target before wrapper rewriting begins.
    let journal = if change.current_path.exists() {
        &change.current_path
    } else {
        &change.target_path
    };
    legacy_manifest_evidence(&manifest_path(journal))
}

fn rewrite(change: &JournalChange) -> Result<PathBuf, RewriteError> {
    let sol_alias = change.alias.clone();
    let journal_alias = sol_alias.with_file_name("journal");
    let sol_content = fs::read_to_string(&sol_alias).map_err(|e| {
        RewriteError::Refusal(format!(
            "journal config: refused: cannot read {}: {e}",
            sol_alias.display()
        ))
    })?;
    let Some(sol_wrapper) = parse_wrapper(&sol_content) else {
        return Err(RewriteError::Refusal(wrapper_refusal(&sol_alias)));
    };
    let sol_bin = sol_wrapper.sol_bin;
    let journal_bin = if !journal_alias.exists() && !journal_alias.is_symlink() {
        sol_bin.with_file_name("journal")
    } else if journal_alias.is_symlink() {
        return Err(RewriteError::Refusal(wrapper_refusal(&journal_alias)));
    } else {
        let content = fs::read_to_string(&journal_alias).map_err(|e| {
            RewriteError::Refusal(format!(
                "journal config: refused: cannot read {}: {e}",
                journal_alias.display()
            ))
        })?;
        match parse_wrapper(&content) {
            Some(wrapper) => wrapper.sol_bin,
            None => return Err(RewriteError::Refusal(wrapper_refusal(&journal_alias))),
        }
    };
    let root_token = root_token_from_path(&change.identity_root).map_err(|error| {
        RewriteError::Refusal(format!("journal config: identity root refused: {error}"))
    })?;
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    let admission = admit_setup(SetupAdmissionRequest {
        owner: OwnerBase::at_home(change.home_dir.clone(), PlatformTag::current()).map_err(
            |error| {
                RewriteError::Refusal(format!("journal config: identity owner refused: {error}"))
            },
        )?,
        root_token,
        journal_token: journal_token_from_path(&change.target_path).map_err(|error| {
            RewriteError::Refusal(format!("journal config: journal path refused: {error}"))
        })?,
        journal_is_explicit: true,
        legacy_manifest: legacy_manifest_for_rewrite(change),
        artifacts: gather_wrapper_artifact_evidence(&change.home_dir, &namespace),
    })
    .map_err(|error| {
        RewriteError::Refusal(format!(
            "journal config: identity admission refused: {error}"
        ))
    })?;
    let guard = solstone_core_installation_identity::GuardFields::from_binding(admission.binding());
    let _wrapper_lock =
        wrapper_lock(&change.home_dir).map_err(|error| RewriteError::Install(error.to_string()))?;
    write_wrappers_atomically(&[
        (
            sol_alias,
            render_wrapper("solstone", &change.target_path, &sol_bin, &guard),
        ),
        (
            journal_alias,
            render_wrapper("journal", &change.target_path, &journal_bin, &guard),
        ),
    ])
    .map_err(|error| RewriteError::Install(error.to_string()))?;
    Ok(journal_bin)
}
fn execute(c: &JournalChange, d: &Decision, service: &dyn ServiceCommandRunner) -> u8 {
    if c.action == Some(RequestedAction::Force) {
        eprintln!(
            "journal config: warning: --force bypasses confirmation and target activity checks"
        );
    }
    match d.action {
        Action::Merge => {
            println!("{}", d.message.as_ref().unwrap());
            1
        }
        Action::Refuse => {
            eprintln!("{}", d.message.as_ref().unwrap());
            1
        }
        Action::Noop => {
            println!(
                "journal config: journal already set to {}",
                c.target_path.display()
            );
            0
        }
        _ if d.plan_only => {
            println!("{}", plan(c, d));
            d.exit_code
        }
        Action::Proceed | Action::Switch => run_switch(c, service),
        Action::Move => run_move(c, service),
    }
}
fn run_switch(c: &JournalChange, service: &dyn ServiceCommandRunner) -> u8 {
    if let Err(e) = fs::create_dir_all(&c.target_path) {
        eprintln!(
            "journal config: refused: cannot create {}: {e}",
            c.target_path.display()
        );
        return 1;
    }
    let restart = match rewrite(c) {
        Ok(v) => v,
        Err(RewriteError::Refusal(message)) => {
            eprintln!("{message}");
            return 1;
        }
        Err(RewriteError::Install(e)) => {
            eprintln!(
                "journal config: refused: cannot rewrite {}: {e}",
                c.alias.display()
            );
            return 1;
        }
    };
    if !c.service_installed {
        println!("service not installed; wrapper updated.");
        return 0;
    }
    if !c.service_running {
        println!("service installed but not running; wrapper updated.");
        return 0;
    }
    match service.run(&restart, ServiceCommand::RestartIfInstalled) {
        ServiceCommandResult::Exited { code: 0 } => {
            println!("wrapper updated; service restarted.");
            0
        }
        ServiceCommandResult::ExecutableMissing { error } => {
            eprintln!(
                "journal config: wrapper rewritten to {} but journal service restart could not run ({error}); restart manually",
                c.target_path.display()
            );
            2
        }
        ServiceCommandResult::Exited { code } => {
            eprintln!(
                "journal config: wrapper rewritten to {} but 'journal service restart --if-installed' exited {code}; investigate and restart manually",
                c.target_path.display()
            );
            2
        }
        ServiceCommandResult::LaunchError { error } => {
            eprintln!(
                "journal config: wrapper rewritten to {} but journal service restart could not run ({error}); restart manually",
                c.target_path.display()
            );
            2
        }
    }
}
fn maybe_restart_current_service(c: &JournalChange, service: &dyn ServiceCommandRunner) {
    if !c.service_running {
        return;
    }
    if let ServiceCommandResult::ExecutableMissing { error } =
        service.run(&c.service_bin, ServiceCommand::Start)
    {
        eprintln!("journal config: rollback warning: could not restart service ({error})");
    }
}
fn run_move(c: &JournalChange, service: &dyn ServiceCommandRunner) -> u8 {
    if !c.target_parent_exists {
        eprintln!("{}", missing_parent(c));
        return 1;
    }
    if !c.current_path.exists() {
        eprintln!("{}", missing_current(c));
        return 1;
    }
    if c.target_path.exists() || c.target_path.is_symlink() {
        eprintln!("{}", existing_target(c));
        return 1;
    }
    if c.same_filesystem == Some(false) {
        eprintln!("{}", cross_filesystem(c));
        return 1;
    }
    if c.service_running {
        match service.run(&c.service_bin, ServiceCommand::Stop) {
            ServiceCommandResult::Exited { code: 0 } => {}
            ServiceCommandResult::ExecutableMissing { error }
            | ServiceCommandResult::LaunchError { error } => {
                eprintln!("journal config: could not stop service before move ({error})");
                return 2;
            }
            ServiceCommandResult::Exited { .. } => {
                eprintln!("journal config: could not stop service before move");
                return 2;
            }
        }
    }
    if let Err(e) = fs::rename(&c.current_path, &c.target_path) {
        maybe_restart_current_service(c, service);
        eprintln!("journal config: move failed: {e}");
        return 1;
    }
    let restart = match rewrite(c) {
        Ok(v) => v,
        Err(RewriteError::Refusal(message)) => {
            eprintln!("{message}");
            let _ = fs::rename(&c.target_path, &c.current_path);
            maybe_restart_current_service(c, service);
            return 1;
        }
        Err(RewriteError::Install(e)) => {
            let restored = fs::rename(&c.target_path, &c.current_path).is_ok();
            maybe_restart_current_service(c, service);
            let suffix = if restored {
                "; restored original journal"
            } else {
                ""
            };
            eprintln!("journal config: move failed during wrapper update: {e}{suffix}");
            return 2;
        }
    };
    if !c.service_installed {
        println!("service not installed; journal moved; wrapper updated.");
        return 0;
    }
    if !c.service_running {
        println!("service installed but not running; journal moved; wrapper updated.");
        return 0;
    }
    match service.run(&restart, ServiceCommand::Start) {
        ServiceCommandResult::Exited { code: 0 } => {
            println!("journal moved; wrapper updated; service restarted.");
            0
        }
        _ => {
            eprintln!(
                "wrapper updated to {} but service start failed; restart manually",
                c.target_path.display()
            );
            2
        }
    }
}
pub(crate) fn run(command: ConfigCommand) -> ExitCode {
    match command {
        ConfigCommand::Show => show(),
        ConfigCommand::Journal(options) => journal(options),
    }
}
fn wrapper_status(alias: &Path) -> (&'static str, Option<String>) {
    if !alias.exists() && !alias.is_symlink() {
        ("absent", None)
    } else if alias.is_symlink() {
        ("legacy-symlink", None)
    } else {
        fs::read_to_string(alias)
            .ok()
            .and_then(|text| parse_wrapper(&text))
            .map_or(("foreign", None), |wrapper| {
                ("managed", Some(wrapper.journal))
            })
    }
}
fn show_source(source: Source, embedded: Option<&str>, env_journal: Option<&str>) -> &'static str {
    match source {
        Source::Env if embedded == env_journal => "wrapper-embedded",
        Source::Env => "caller-override",
        Source::Config => "user config (~/.config/solstone/config.toml)",
        Source::Default => "built-in default (~/journal)",
        Source::Source => "source-tree fallback",
    }
}
fn show() -> ExitCode {
    let h = home();
    let wrapper = wrapper_status(&wrapper_paths(&h).solstone);
    let cfg = read_config_journal(&h).ok().flatten();
    let root = env::current_dir()
        .ok()
        .and_then(|p| detect_checkout_root(&p));
    let resolved = resolve_journal_path(
        env::var_os("SOLSTONE_JOURNAL").as_deref(),
        cfg.as_deref(),
        root.as_deref(),
        &h,
    );
    let source = show_source(
        resolved.source,
        wrapper.1.as_deref(),
        env::var("SOLSTONE_JOURNAL").ok().as_deref(),
    );
    println!(
        "path: {}\nsource: {}\nwrapper-status: {}",
        resolved.path.display(),
        source,
        wrapper.0
    );
    ExitCode::SUCCESS
}
fn journal(o: ConfigJournalOptions) -> ExitCode {
    let target = resolve_non_strict(Path::new(&o.path));
    if o.path
        .chars()
        .any(|c| matches!(c, '$' | '`' | '"' | '\\' | '\n'))
    {
        eprintln!(
            "journal config: refused: journal path contains shell-active character: {:?}",
            o.path
        );
        return ExitCode::from(1);
    }
    if let Some(root) = project_root()
        && target == root.join("journal")
        && !is_source_checkout()
    {
        eprintln!(
            "journal config: refused: {} is the source-tree fallback path but this is not a source checkout",
            target.display()
        );
        return ExitCode::from(1);
    }
    if o.action == Some(ConfigAction::Move) && !target.parent().is_some_and(Path::exists) {
        eprintln!(
            "journal config: refused: move target parent does not exist: {}",
            target.parent().unwrap().display()
        );
        return ExitCode::from(1);
    }
    let owner_home = home();
    let alias = wrapper_paths(&owner_home).solstone;
    if !alias.exists() || alias.is_symlink() {
        eprintln!("{}", wrapper_refusal(&alias));
        return ExitCode::from(1);
    }
    let content = match fs::read_to_string(&alias) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "journal config: refused: cannot read {}: {e}",
                alias.display()
            );
            return ExitCode::from(1);
        }
    };
    let Some(wrapper) = parse_wrapper(&content) else {
        eprintln!("{}", wrapper_refusal(&alias));
        return ExitCode::from(1);
    };
    let current = resolve_non_strict(Path::new(&wrapper.journal));
    let current_active = match active(&current) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let target_active = match active(&target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let current_device = fs::metadata(&current).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.dev()
    });
    let target_parent_device = target.parent().and_then(|p| fs::metadata(p).ok()).map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.dev()
    });
    let identity_root = match identity_root_from_current_executable() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let c = JournalChange {
        paths_equal: current == target,
        current_exists: current.exists(),
        target_exists: target.exists() || target.is_symlink(),
        target_parent_exists: target.parent().is_some_and(Path::exists),
        same_filesystem: current_device
            .zip(target_parent_device)
            .map(|(a, b)| a == b),
        current_device,
        target_parent_device,
        service_installed: installed(),
        service_running: running(),
        action: o.action.map(Into::into),
        yes: o.yes,
        dry_run: o.dry_run,
        service_bin: wrapper.sol_bin.with_file_name("journal"),
        sol_bin: wrapper.sol_bin,
        alias,
        home_dir: owner_home,
        identity_root,
        current_path: current,
        target_path: target,
        current_active,
        target_active,
    };
    let d = decide(&c);
    ExitCode::from(execute(&c, &d, &RealServiceRunner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::ops::Deref;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct FakeServiceRunner {
        results: Mutex<VecDeque<ServiceCommandResult>>,
        calls: Mutex<Vec<(PathBuf, ServiceCommand)>>,
    }
    impl FakeServiceRunner {
        fn new(results: impl IntoIterator<Item = ServiceCommandResult>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    impl ServiceCommandRunner for FakeServiceRunner {
        fn run(&self, executable: &Path, command: ServiceCommand) -> ServiceCommandResult {
            self.calls
                .lock()
                .unwrap()
                .push((executable.to_path_buf(), command));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ServiceCommandResult::Exited { code: 0 })
        }
    }

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(label: &str) -> Self {
            let root = PathBuf::from("/var/tmp").join(format!(
                "config-{label}-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Deref for TestRoot {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn test_root(label: &str) -> TestRoot {
        TestRoot::new(label)
    }

    fn move_change(root: &Path) -> JournalChange {
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(source.join("health")).unwrap();
        fs::write(
            source.join("health/setup-state.json"),
            r#"{"schema_version":1,"started_at":"2026-01-01T00:00:00Z","completed_at":null,"mode":"setup","args_resolved":{},"steps":[]}"#,
        )
        .unwrap();
        let alias = root.join(".local/bin/solstone");
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        fs::write(
            &alias,
            legacy_wrapper("solstone", &source, Path::new("/service/solstone")),
        )
        .unwrap();
        fs::write(
            alias.with_file_name("journal"),
            legacy_wrapper("journal", &source, Path::new("/restart/journal")),
        )
        .unwrap();
        let mut c = change(Some(RequestedAction::Move));
        c.current_path = source;
        c.target_path = target;
        c.alias = alias;
        c.sol_bin = PathBuf::from("/service/solstone");
        c.service_bin = PathBuf::from("/service/journal");
        c.home_dir = root.to_path_buf();
        c.identity_root = root.to_path_buf();
        c.yes = true;
        c
    }

    fn legacy_wrapper(binary: &str, journal: &Path, sol_bin: &Path) -> String {
        format!(
            "#!/bin/bash\n# {binary} — managed by 'journal config'. Edits will be overwritten.\n# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:={}}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='{}'\nexec \"$SOL_BIN\" \"$@\"\n",
            journal.display(),
            sol_bin.display()
        )
    }

    fn change(action: Option<RequestedAction>) -> JournalChange {
        JournalChange {
            current_path: PathBuf::from("/current"),
            target_path: PathBuf::from("/target"),
            paths_equal: false,
            current_active: false,
            target_active: false,
            current_exists: true,
            target_exists: false,
            target_parent_exists: true,
            current_device: Some(1),
            target_parent_device: Some(1),
            same_filesystem: Some(true),
            service_installed: false,
            service_running: false,
            action,
            yes: false,
            dry_run: false,
            sol_bin: PathBuf::from("/bin/solstone"),
            service_bin: PathBuf::from("/bin/journal"),
            alias: PathBuf::from("/home/.local/bin/solstone"),
            home_dir: PathBuf::from("/home"),
            identity_root: PathBuf::from("/identity-root"),
        }
    }

    #[test]
    fn decide_preserves_reference_order_and_dry_run_landmines() {
        let mut c = change(Some(RequestedAction::Merge));
        assert_eq!(decide(&c).action, Action::Merge);
        c.paths_equal = true;
        assert_eq!(decide(&c).action, Action::Merge);
        c.action = None;
        assert_eq!(decide(&c).action, Action::Noop);
        c.paths_equal = false;
        c.dry_run = true;
        assert_eq!(decide(&c).action, Action::Proceed);
        c.action = Some(RequestedAction::Force);
        assert_eq!(decide(&c).action, Action::Switch);
        c.action = Some(RequestedAction::Switch);
        assert!(decide(&c).plan_only);
    }

    #[test]
    fn decide_covers_every_move_and_switch_branch() {
        let mut c = change(Some(RequestedAction::Move));
        c.target_parent_exists = false;
        assert_eq!(decide(&c).message, Some(missing_parent(&c)));
        c.target_parent_exists = true;
        c.current_exists = false;
        assert_eq!(decide(&c).message, Some(missing_current(&c)));
        c.current_exists = true;
        c.target_exists = true;
        assert_eq!(decide(&c).message, Some(existing_target(&c)));
        c.target_exists = false;
        c.target_active = true;
        assert!(
            decide(&c)
                .message
                .unwrap()
                .contains("--move requires a not active target")
        );
        c.target_active = false;
        c.same_filesystem = Some(false);
        c.current_device = Some(1);
        c.target_parent_device = Some(2);
        assert_eq!(
            decide(&c).message,
            Some("journal config: refused: cannot move across filesystems (current device=1, target parent device=2); archive merge is temporarily unavailable, so keep both journal copies".to_owned())
        );
        c.same_filesystem = Some(true);
        c.dry_run = true;
        assert_eq!(
            decide(&c),
            Decision {
                action: Action::Move,
                exit_code: 0,
                message: None,
                plan_only: true
            }
        );
        c.dry_run = false;
        assert_eq!(
            decide(&c),
            Decision {
                action: Action::Move,
                exit_code: 1,
                message: None,
                plan_only: true
            }
        );
        c.yes = true;
        assert_eq!(decide(&c), decision(Action::Move, 0));
        c.action = Some(RequestedAction::Switch);
        c.yes = false;
        assert_eq!(
            decide(&c),
            Decision {
                action: Action::Switch,
                exit_code: 1,
                message: None,
                plan_only: true
            }
        );
        c.dry_run = true;
        assert_eq!(
            decide(&c),
            Decision {
                action: Action::Switch,
                exit_code: 0,
                message: None,
                plan_only: true
            }
        );
        c.dry_run = false;
        c.yes = true;
        assert_eq!(decide(&c), decision(Action::Switch, 0));
    }

    #[test]
    fn run_move_covers_the_eleven_reference_exit_sites() {
        let no_service = FakeServiceRunner::new([]);

        let root = test_root("move-parent");
        let mut c = move_change(&root);
        c.target_parent_exists = false;
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(c.current_path.exists());

        let root = test_root("move-source");
        let c = move_change(&root);
        fs::remove_dir_all(&c.current_path).unwrap();
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(!c.current_path.exists());

        let root = test_root("move-target");
        let c = move_change(&root);
        fs::create_dir_all(&c.target_path).unwrap();
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(c.current_path.exists());
        assert!(c.target_path.exists());

        let root = test_root("move-device");
        let mut c = move_change(&root);
        c.same_filesystem = Some(false);
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(c.current_path.exists());

        let root = test_root("move-stop-missing");
        let mut c = move_change(&root);
        c.service_running = true;
        let missing = FakeServiceRunner::new([ServiceCommandResult::ExecutableMissing {
            error: io::Error::new(io::ErrorKind::NotFound, "stop"),
        }]);
        assert_eq!(run_move(&c, &missing), 2);
        assert!(c.current_path.exists());

        let root = test_root("move-stop-nonzero");
        let mut c = move_change(&root);
        c.service_running = true;
        let nonzero = FakeServiceRunner::new([ServiceCommandResult::Exited { code: 1 }]);
        assert_eq!(run_move(&c, &nonzero), 2);
        assert!(c.current_path.exists());

        let root = test_root("move-rename");
        let mut c = move_change(&root);
        let not_a_directory = root.join("not-a-directory");
        fs::write(&not_a_directory, "file").unwrap();
        c.target_path = not_a_directory.join("target");
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(c.current_path.exists());

        let root = test_root("move-wrapper-refusal");
        let c = move_change(&root);
        fs::write(&c.alias, "foreign wrapper").unwrap();
        assert_eq!(run_move(&c, &no_service), 1);
        assert!(c.current_path.exists());
        assert!(!c.target_path.exists());

        let root = test_root("move-start-missing");
        let mut c = move_change(&root);
        c.service_installed = true;
        c.service_running = true;
        let start_missing = FakeServiceRunner::new([
            ServiceCommandResult::Exited { code: 0 },
            ServiceCommandResult::ExecutableMissing {
                error: io::Error::new(io::ErrorKind::NotFound, "start"),
            },
        ]);
        assert_eq!(run_move(&c, &start_missing), 2);
        assert!(!c.current_path.exists());
        assert!(c.target_path.exists());

        let root = test_root("move-start-nonzero");
        let mut c = move_change(&root);
        c.service_installed = true;
        c.service_running = true;
        let start_nonzero = FakeServiceRunner::new([
            ServiceCommandResult::Exited { code: 0 },
            ServiceCommandResult::Exited { code: 1 },
        ]);
        assert_eq!(run_move(&c, &start_nonzero), 2);
        assert!(!c.current_path.exists());
        assert!(c.target_path.exists());
    }

    #[test]
    fn move_uses_service_bin_to_stop_and_journal_wrapper_bin_to_start() {
        let root = test_root("service-bins");
        let mut c = move_change(&root);
        c.service_installed = true;
        c.service_running = true;
        let service = FakeServiceRunner::new([
            ServiceCommandResult::Exited { code: 0 },
            ServiceCommandResult::Exited { code: 0 },
        ]);
        assert_eq!(run_move(&c, &service), 0);
        assert_eq!(
            *service.calls.lock().unwrap(),
            vec![
                (PathBuf::from("/service/journal"), ServiceCommand::Stop),
                (PathBuf::from("/restart/journal"), ServiceCommand::Start),
            ]
        );
    }

    #[test]
    fn moved_legacy_manifest_remains_admission_evidence() {
        let root = test_root("moved-legacy-manifest");
        let c = move_change(&root);
        fs::rename(&c.current_path, &c.target_path).expect("move legacy journal");
        assert!(matches!(
            legacy_manifest_for_rewrite(&c),
            solstone_core_installation_identity::LegacyManifestEvidence::ValidProviderlessSchemaV1
        ));
    }

    #[test]
    fn move_failures_restart_a_service_stopped_before_the_move() {
        let root = test_root("move-restart-rename");
        let mut c = move_change(&root);
        c.service_running = true;
        let blocked_parent = root.join("not-a-directory");
        fs::write(&blocked_parent, "file").unwrap();
        c.target_path = blocked_parent.join("target");
        let service = FakeServiceRunner::new([
            ServiceCommandResult::Exited { code: 0 },
            ServiceCommandResult::Exited { code: 0 },
        ]);
        assert_eq!(run_move(&c, &service), 1);
        assert_eq!(
            *service.calls.lock().unwrap(),
            vec![
                (PathBuf::from("/service/journal"), ServiceCommand::Stop),
                (PathBuf::from("/service/journal"), ServiceCommand::Start),
            ]
        );

        let root = test_root("move-restart-refusal");
        let mut c = move_change(&root);
        c.service_running = true;
        fs::write(c.alias.with_file_name("journal"), "foreign wrapper").unwrap();
        let service = FakeServiceRunner::new([
            ServiceCommandResult::Exited { code: 0 },
            ServiceCommandResult::Exited { code: 0 },
        ]);
        assert_eq!(run_move(&c, &service), 1);
        assert_eq!(service.calls.lock().unwrap().len(), 2);
        assert_eq!(service.calls.lock().unwrap()[1].1, ServiceCommand::Start);
    }

    #[test]
    fn rewrite_reports_the_failing_journal_alias() {
        let root = test_root("rewrite-journal-alias");
        let c = move_change(&root);
        let journal_alias = c.alias.with_file_name("journal");
        for kind in ["foreign", "symlink", "unreadable"] {
            let _ = fs::remove_file(&journal_alias);
            let _ = fs::remove_dir(&journal_alias);
            match kind {
                "foreign" => fs::write(&journal_alias, "foreign wrapper").unwrap(),
                "symlink" => std::os::unix::fs::symlink("old-journal", &journal_alias).unwrap(),
                "unreadable" => fs::create_dir(&journal_alias).unwrap(),
                _ => unreachable!(),
            }
            let error = rewrite(&c).unwrap_err();
            let message = match error {
                RewriteError::Refusal(message) => message,
                RewriteError::Install(_) => panic!("journal alias must be rejected before install"),
            };
            assert!(
                message.contains(&journal_alias.display().to_string()),
                "{kind}: {message}"
            );
            assert!(
                !message.contains(&c.alias.display().to_string()),
                "{kind}: {message}"
            );
            if kind == "unreadable" {
                assert!(message.starts_with("journal config: refused: cannot read"));
            } else {
                assert!(message.contains("is not a managed wrapper"));
            }
        }
    }

    #[test]
    fn plan_matches_all_six_reference_service_summaries() {
        for (action, installed, running, expected) in [
            (
                Action::Move,
                false,
                false,
                "service: not installed; will move and rewrite wrapper",
            ),
            (
                Action::Move,
                true,
                false,
                "service: installed but not running; will move and rewrite wrapper",
            ),
            (
                Action::Move,
                true,
                true,
                "service: installed and running; will stop, move, rewrite wrapper, restart",
            ),
            (
                Action::Switch,
                false,
                false,
                "service: not installed; will rewrite wrapper",
            ),
            (
                Action::Switch,
                true,
                false,
                "service: installed but not running; will rewrite wrapper",
            ),
            (
                Action::Switch,
                true,
                true,
                "service: installed and running; will rewrite wrapper, restart",
            ),
        ] {
            let mut c = change(None);
            c.service_installed = installed;
            c.service_running = running;
            let d = decision(action, 0);
            assert_eq!(service_summary(&c, &d), expected);
        }

        let c = change(Some(RequestedAction::Switch));
        let d = decision(Action::Switch, 1);
        assert_eq!(
            plan(&c, &d),
            "journal config journal - plan summary\n\ncurrent: /current (not active)\ntarget:  /target (not active)\naction:  switch\nservice: not installed; will rewrite wrapper\n\ncurrent journal is left intact. to re-adopt it later: journal config journal /current --switch --yes\n\nre-run with --yes to proceed"
        );

        let c = change(Some(RequestedAction::Move));
        let d = decision(Action::Move, 0);
        assert_eq!(
            plan(&c, &d),
            "journal config journal - plan summary\n\ncurrent: /current (not active)\ntarget:  /target (not active)\naction:  move\nservice: not installed; will move and rewrite wrapper\nfilesystem: same device\n\nre-run with --yes to proceed"
        );
    }

    #[test]
    fn show_source_covers_all_reference_labels() {
        assert_eq!(
            show_source(Source::Env, Some("/wrapper"), Some("/wrapper")),
            "wrapper-embedded"
        );
        assert_eq!(
            show_source(Source::Env, Some("/wrapper"), Some("/caller")),
            "caller-override"
        );
        assert_eq!(
            show_source(Source::Config, None, None),
            "user config (~/.config/solstone/config.toml)"
        );
        assert_eq!(
            show_source(Source::Default, None, None),
            "built-in default (~/journal)"
        );
        assert_eq!(
            show_source(Source::Source, None, None),
            "source-tree fallback"
        );
    }

    #[test]
    fn wrapper_status_covers_all_reference_states() {
        let root = test_root("wrapper-status");
        let alias = root.join("sol");
        assert_eq!(wrapper_status(&alias), ("absent", None));

        std::os::unix::fs::symlink("old-sol", &alias).unwrap();
        assert_eq!(wrapper_status(&alias), ("legacy-symlink", None));
        fs::remove_file(&alias).unwrap();

        fs::write(&alias, "not a wrapper").unwrap();
        assert_eq!(wrapper_status(&alias), ("foreign", None));
        fs::remove_file(&alias).unwrap();
        fs::create_dir(&alias).unwrap();
        assert_eq!(wrapper_status(&alias), ("foreign", None));
        fs::remove_dir(&alias).unwrap();

        let legacy = legacy_wrapper("solstone", Path::new("/legacy"), Path::new("/bin/solstone"))
            .replace("# managed-version: 7", "# managed-version: 5");
        fs::write(&alias, legacy).unwrap();
        assert_eq!(
            wrapper_status(&alias),
            ("managed", Some("/legacy".to_owned()))
        );
    }

    #[test]
    fn native_resolution_has_no_unconfigured_error_state() {
        let resolved = resolve_journal_path(None, None, None, Path::new("/home/owner"));
        assert_eq!(resolved.source, Source::Default);
        assert_eq!(resolved.path, PathBuf::from("/home/owner/journal"));
        assert_eq!(
            show_source(resolved.source, None, None),
            "built-in default (~/journal)"
        );
    }

    #[test]
    fn rewrite_always_emits_the_current_wrapper_marker() {
        let root = test_root("wrapper-version");
        let c = move_change(&root);
        assert_eq!(run_move(&c, &FakeServiceRunner::new([])), 0);
        assert!(
            fs::read_to_string(&c.alias)
                .unwrap()
                .contains("# managed-version: 8")
        );
    }

    #[test]
    fn non_strict_resolution_matches_pathlib_for_relative_home_and_missing_paths() {
        let root = PathBuf::from("/var/tmp").join(format!("config-resolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cwd = root.join("cwd");
        let home = root.join("home");
        fs::create_dir_all(cwd.join("existing")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let cwd = root.join("cwd");
        let home = root.join("home");

        assert_eq!(
            resolve_non_strict_from(Path::new("existing/../relative"), &home, &cwd),
            cwd.join("relative")
        );
        assert_eq!(
            resolve_non_strict_from(Path::new("~/journal"), &home, &cwd),
            home.join("journal")
        );
        assert_eq!(
            resolve_non_strict_from(Path::new("existing/new-leaf"), &home, &cwd),
            cwd.join("existing/new-leaf")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn move_refusal_is_strict_but_plan_collapses_indeterminate() {
        let mut c = change(Some(RequestedAction::Move));
        c.dry_run = true;
        c.same_filesystem = None;
        let d = decide(&c);
        assert_eq!(d.action, Action::Move);
        assert!(plan(&c, &d).contains("filesystem: different devices"));
        c.same_filesystem = Some(false);
        assert_eq!(decide(&c).action, Action::Refuse);
    }

    #[test]
    fn parser_accepts_all_historical_marker_versions() {
        for version in 1..=8 {
            let content = format!(
                "# managed-version: {version}\n: \"${{SOLSTONE_JOURNAL:=/journal}}\"\nSOL_BIN='/bin/it'\\''s'\n"
            );
            let wrapper = parse_wrapper(&content).unwrap();
            assert_eq!(wrapper.journal, "/journal");
            assert_eq!(wrapper.sol_bin, PathBuf::from("/bin/it's"));
            assert_eq!(wrapper.version, version);
        }
        assert!(parse_wrapper("# managed-version: 9\nSOL_BIN='/bin/sol'\n").is_none());
    }

    #[test]
    fn active_distinguishes_missing_and_corrupt() {
        let root = PathBuf::from("/var/tmp").join(format!("config-active-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(active(&root), Ok(false));
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/journal.json"), "[]").unwrap();
        assert!(
            active(&root)
                .unwrap_err()
                .starts_with("I couldn't read your settings file at ")
        );
        let _ = fs::remove_dir_all(root);
    }
}
