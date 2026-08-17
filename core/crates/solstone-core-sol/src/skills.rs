// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_sol_client::command::CommandOutput;

use super::{EXIT_CONFIG, resolve_project_root};

const EXIT_ARGPARSE_USAGE: i32 = 2;
const USAGE: &str = "Usage: sol skills <install|uninstall|list> [args...]\n";
const ALL_AGENTS: &str = "all";
const PROJECT_MULTI_AGENT: &str = "agents";
const PROJECT_CLAUDE_SKILLS_REL: &str = ".claude/skills";
const PROJECT_AGENTS_SKILLS_REL: &str = ".agents/skills";
const USER_SKILL_NAME: &str = "sol";
const GLOBAL_SKIP_MESSAGE: &str =
    "no AI coding agent config directories found — skipping skill registration";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Claude,
    Codex,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
struct AgentSpec {
    name: &'static str,
    parent_dir: &'static str,
    skills_dir: &'static str,
    silent_when_default_all: bool,
}

impl Agent {
    fn spec(self) -> AgentSpec {
        match self {
            Agent::Claude => AgentSpec {
                name: "claude",
                parent_dir: ".claude",
                skills_dir: PROJECT_CLAUDE_SKILLS_REL,
                silent_when_default_all: false,
            },
            Agent::Codex => AgentSpec {
                name: "codex",
                parent_dir: ".codex",
                skills_dir: ".codex/skills",
                silent_when_default_all: false,
            },
            Agent::Gemini => AgentSpec {
                name: "gemini",
                parent_dir: ".gemini",
                skills_dir: ".gemini/skills",
                silent_when_default_all: true,
            },
        }
    }
}

const USER_AGENT_ORDER: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Gemini];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSelection {
    All,
    One(Agent),
}

impl AgentSelection {
    fn parse(value: &OsStr) -> Result<Self, String> {
        if value == OsStr::new(ALL_AGENTS) {
            return Ok(Self::All);
        }
        if value == OsStr::new("claude") {
            return Ok(Self::One(Agent::Claude));
        }
        if value == OsStr::new("codex") {
            return Ok(Self::One(Agent::Codex));
        }
        if value == OsStr::new("gemini") {
            return Ok(Self::One(Agent::Gemini));
        }
        Err(format!("unsupported --agent {}", value.to_string_lossy()))
    }

    fn user_agents(self) -> (Vec<AgentSpec>, bool) {
        match self {
            Self::All => (
                USER_AGENT_ORDER.into_iter().map(Agent::spec).collect(),
                true,
            ),
            Self::One(agent) => (vec![agent.spec()], false),
        }
    }
}

#[derive(Debug, Clone)]
enum SkillsCommand {
    Install(InstallArgs),
    Uninstall(InstallArgs),
    List(ListArgs),
    Help,
}

#[derive(Debug, Clone)]
struct InstallArgs {
    agent: AgentSelection,
    project: Option<OsString>,
}

#[derive(Debug, Clone)]
struct ListArgs {
    project: Option<OsString>,
}

#[derive(Debug, Clone)]
struct RuntimeContext {
    home: PathBuf,
    cwd: PathBuf,
    project_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Error,
    Installed,
    Noop,
    Removed,
    Replaced,
    Skipped,
    Warning,
}

#[derive(Debug, Clone)]
struct ActionRow {
    agent: String,
    skill: String,
    action: Action,
    path: PathBuf,
    reason: Option<String>,
}

#[derive(Debug, Default)]
struct InstallReport {
    rows: Vec<ActionRow>,
}

impl InstallReport {
    fn error_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.action == Action::Error)
            .count()
    }

    fn all_skipped(&self) -> bool {
        !self.rows.is_empty()
            && self.rows.iter().all(|row| {
                row.action == Action::Skipped
                    && row
                        .reason
                        .as_deref()
                        .is_some_and(|reason| reason.starts_with("config dir absent at "))
            })
    }
}

#[derive(Debug)]
struct StatusRow {
    agent: String,
    skill: String,
    state: &'static str,
}

pub(crate) fn run(args: &[OsString]) -> CommandOutput {
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(message) => return usage_error(message),
    };
    if matches!(command, SkillsCommand::Help) {
        return CommandOutput::success(USAGE);
    }
    let context = match real_context() {
        Ok(context) => context,
        Err(message) => {
            return CommandOutput {
                stdout: String::new(),
                stderr: message,
                exit: i32::from(EXIT_CONFIG),
            };
        }
    };
    run_with_context(command, &context)
}

fn run_with_context(command: SkillsCommand, context: &RuntimeContext) -> CommandOutput {
    match execute(command, context) {
        Ok(output) => output,
        Err(message) => CommandOutput::failure(format!("error: {message}\n"), 1),
    }
}

fn execute(command: SkillsCommand, context: &RuntimeContext) -> Result<CommandOutput, String> {
    match command {
        SkillsCommand::Install(args) => {
            if let Some(project) = args.project {
                let target = resolve_project_target(&project, &context.cwd, &context.home);
                let report = install_project(&context.project_root, &target, args.agent)?;
                Ok(report_output(report, "install"))
            } else {
                let skill_dir = resolve_user_skill(&context.project_root)?;
                let report = install_user(&skill_dir, &context.home, args.agent);
                Ok(report_output(report, "install"))
            }
        }
        SkillsCommand::Uninstall(args) => {
            if let Some(project) = args.project {
                let target = resolve_project_target(&project, &context.cwd, &context.home);
                let report = uninstall_project(&context.project_root, &target, args.agent)?;
                Ok(report_output(report, "uninstall"))
            } else {
                let skill_dir = resolve_user_skill(&context.project_root)?;
                let report = uninstall_user(&skill_dir, &context.home, args.agent);
                Ok(report_output(report, "uninstall"))
            }
        }
        SkillsCommand::List(args) => {
            let rows = if let Some(project) = args.project {
                let target = resolve_project_target(&project, &context.cwd, &context.home);
                list_project_status(&context.project_root, &target)?
            } else {
                let skill_dir = resolve_user_skill(&context.project_root)?;
                list_user_status(&skill_dir, &context.home)
            };
            Ok(CommandOutput::success(status_output(&rows)))
        }
        SkillsCommand::Help => Ok(CommandOutput::success(USAGE)),
    }
}

fn real_context() -> Result<RuntimeContext, String> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            "sol: native skills home is unavailable. Reinstall solstone and solstone-core.\n"
                .to_string()
        })?;
    let cwd = std::env::current_dir().map_err(|error| {
        format!("sol: native skills cwd is unavailable: {error}. Reinstall solstone and solstone-core.\n")
    })?;
    let root = resolve_project_root().map_err(|error| {
        format!(
            "sol: native skills payload root is unavailable: {error}. Reinstall solstone and solstone-core.\n"
        )
    })?;
    let project_root = canonicalize_project_root_for_skills(root);
    Ok(RuntimeContext {
        home,
        cwd,
        project_root,
    })
}

fn canonicalize_project_root_for_skills(root: PathBuf) -> PathBuf {
    fs::canonicalize(&root).unwrap_or(root)
}

fn parse_command(args: &[OsString]) -> Result<SkillsCommand, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err("missing subcommand".to_string());
    };
    if command == OsStr::new("-h") || command == OsStr::new("--help") {
        return Ok(SkillsCommand::Help);
    }
    if command == OsStr::new("install") {
        return parse_install_args(rest).map(SkillsCommand::Install);
    }
    if command == OsStr::new("uninstall") {
        return parse_install_args(rest).map(SkillsCommand::Uninstall);
    }
    if command == OsStr::new("list") {
        return parse_list_args(rest).map(SkillsCommand::List);
    }
    Err(format!("unknown subcommand {}", command.to_string_lossy()))
}

fn parse_install_args(args: &[OsString]) -> Result<InstallArgs, String> {
    let mut agent = AgentSelection::All;
    let mut project = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == OsStr::new("--agent") {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err("--agent requires a value".to_string());
            };
            agent = AgentSelection::parse(value)?;
        } else if let Some(value) = flag_value(arg, "--agent=") {
            agent = AgentSelection::parse(OsStr::new(value))?;
        } else if arg == OsStr::new("--project") {
            if let Some(next) = args.get(index + 1)
                && !next.to_string_lossy().starts_with('-')
            {
                project = Some(next.clone());
                index += 1;
            } else {
                project = Some(OsString::new());
            }
        } else if let Some(value) = flag_value(arg, "--project=") {
            project = Some(OsString::from(value));
        } else {
            return Err(format!("unrecognized argument {}", arg.to_string_lossy()));
        }
        index += 1;
    }
    Ok(InstallArgs { agent, project })
}

fn parse_list_args(args: &[OsString]) -> Result<ListArgs, String> {
    let mut project = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == OsStr::new("--project") {
            if let Some(next) = args.get(index + 1)
                && !next.to_string_lossy().starts_with('-')
            {
                project = Some(next.clone());
                index += 1;
            } else {
                project = Some(OsString::new());
            }
        } else if let Some(value) = flag_value(arg, "--project=") {
            project = Some(OsString::from(value));
        } else {
            return Err(format!("unrecognized argument {}", arg.to_string_lossy()));
        }
        index += 1;
    }
    Ok(ListArgs { project })
}

fn flag_value<'a>(arg: &'a OsStr, prefix: &str) -> Option<&'a str> {
    arg.to_str()?.strip_prefix(prefix)
}

fn usage_error(message: String) -> CommandOutput {
    CommandOutput {
        stdout: String::new(),
        stderr: format!("{USAGE}sol skills: error: {message}\n"),
        exit: EXIT_ARGPARSE_USAGE,
    }
}

fn resolve_project_target(value: &OsStr, cwd: &Path, home: &Path) -> PathBuf {
    if value.is_empty() {
        return resolve_non_strict(cwd, cwd, home);
    }
    resolve_non_strict(&expand_user(value, home), cwd, home)
}

fn expand_user(value: &OsStr, home: &Path) -> PathBuf {
    let text = value.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn resolve_non_strict(path: &Path, cwd: &Path, _home: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut cursor = absolute.clone();
    let mut missing = Vec::new();
    while !cursor.exists() {
        let Some(name) = cursor.file_name().map(OsString::from) else {
            break;
        };
        missing.push(name);
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent.to_path_buf();
    }
    let mut resolved = fs::canonicalize(&cursor).unwrap_or(cursor);
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    resolved
}

fn resolve_user_skill(project_root: &Path) -> Result<PathBuf, String> {
    let skill_dir = project_root
        .join("solstone")
        .join("talent")
        .join(USER_SKILL_NAME);
    let skill_file = skill_dir.join("SKILL.md");
    if !skill_file.is_file() {
        return Err(format!(
            "expected bundled umbrella skill at solstone/talent/sol/SKILL.md ({})",
            skill_file.display()
        ));
    }
    Ok(skill_dir)
}

fn install_user(skill_dir: &Path, home: &Path, selection: AgentSelection) -> InstallReport {
    let (selected, _default_all) = selection.user_agents();
    let mut report = InstallReport::default();
    for spec in selected {
        let skills_root = home.join(spec.skills_dir);
        if let Err(error) = fs::create_dir_all(&skills_root) {
            append_error(&mut report.rows, spec.name, "", &skills_root, error);
            continue;
        }
        let target = skills_root.join(USER_SKILL_NAME);
        let mut action = Action::Installed;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if let Err(error) = fs::remove_file(&target) {
                    append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                    continue;
                }
                action = Action::Replaced;
            }
            Ok(metadata) if !metadata.file_type().is_dir() => {
                if let Err(error) = fs::remove_file(&target) {
                    append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                    continue;
                }
                action = Action::Replaced;
            }
            Ok(_) => {
                match tree_matches(skill_dir, &target) {
                    Ok(true) => {
                        report.rows.push(ActionRow {
                            agent: spec.name.to_string(),
                            skill: USER_SKILL_NAME.to_string(),
                            action: Action::Noop,
                            path: target,
                            reason: None,
                        });
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                        continue;
                    }
                }
                // Python checked os.access(target, W_OK) before rmtree. Native Rust
                // intentionally omits that pre-check and lets remove_dir_all/write fail;
                // the filesystem outcome is the same, but the reason string is Rust-native.
                if let Err(error) = fs::remove_dir_all(&target) {
                    append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                    continue;
                }
                action = Action::Replaced;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                continue;
            }
        }
        if let Err(error) = copy_tree_files(skill_dir, &target) {
            append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
            continue;
        }
        report.rows.push(ActionRow {
            agent: spec.name.to_string(),
            skill: USER_SKILL_NAME.to_string(),
            action,
            path: target,
            reason: None,
        });
    }
    report
}

fn uninstall_user(skill_dir: &Path, home: &Path, selection: AgentSelection) -> InstallReport {
    let (selected, default_all) = selection.user_agents();
    let mut report = InstallReport::default();
    for spec in selected {
        let parent = home.join(spec.parent_dir);
        if !parent.exists() {
            if default_all && spec.silent_when_default_all {
                continue;
            }
            report.rows.push(ActionRow {
                agent: spec.name.to_string(),
                skill: String::new(),
                action: Action::Skipped,
                path: parent.clone(),
                reason: Some(format!("config dir absent at {}", parent.display())),
            });
            continue;
        }
        let target = home.join(spec.skills_dir).join(
            skill_dir
                .file_name()
                .unwrap_or_else(|| OsStr::new(USER_SKILL_NAME)),
        );
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.rows.push(ActionRow {
                    agent: spec.name.to_string(),
                    skill: USER_SKILL_NAME.to_string(),
                    action: Action::Skipped,
                    path: target,
                    reason: Some("nothing to remove".to_string()),
                });
            }
            Err(error) => {
                append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error)
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
                report.rows.push(ActionRow {
                    agent: spec.name.to_string(),
                    skill: USER_SKILL_NAME.to_string(),
                    action: Action::Error,
                    path: target,
                    reason: Some("refusing to remove non-directory".to_string()),
                });
            }
            Ok(_) => {
                if let Err(error) = fs::remove_dir_all(&target) {
                    append_error(&mut report.rows, spec.name, USER_SKILL_NAME, &target, error);
                    continue;
                }
                report.rows.push(ActionRow {
                    agent: spec.name.to_string(),
                    skill: USER_SKILL_NAME.to_string(),
                    action: Action::Removed,
                    path: target,
                    reason: None,
                });
            }
        }
    }
    report
}

fn project_targets(
    target: &Path,
    selection: AgentSelection,
) -> Result<Vec<(&'static str, PathBuf)>, String> {
    match selection {
        AgentSelection::All => Ok(vec![
            ("claude", target.join(PROJECT_CLAUDE_SKILLS_REL)),
            (PROJECT_MULTI_AGENT, target.join(PROJECT_AGENTS_SKILLS_REL)),
        ]),
        AgentSelection::One(Agent::Claude) => {
            Ok(vec![("claude", target.join(PROJECT_CLAUDE_SKILLS_REL))])
        }
        AgentSelection::One(agent) => Err(format!(
            "--agent {} is not supported with --project; use --agent all or --agent claude",
            agent.spec().name
        )),
    }
}

fn install_project(
    project_root: &Path,
    target: &Path,
    selection: AgentSelection,
) -> Result<InstallReport, String> {
    let sources = solstone_core_skill_state::discover_project_sources(project_root)?;
    let source_names = sources
        .iter()
        .filter_map(|source| source.file_name().map(OsString::from))
        .collect::<Vec<_>>();
    let mut report = InstallReport::default();
    for (agent, link_parent) in project_targets(target, selection)? {
        if let Err(error) = fs::create_dir_all(&link_parent) {
            append_error(&mut report.rows, agent, "", &link_parent, error);
            continue;
        }
        for source in &sources {
            install_project_source(agent, source, &link_parent, &mut report);
        }
        remove_stale_project_links(agent, &link_parent, &source_names, &mut report);
    }
    Ok(report)
}

fn install_project_source(
    agent: &str,
    source: &Path,
    link_parent: &Path,
    report: &mut InstallReport,
) {
    let name = source
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let link = link_parent.join(name);
    let target = solstone_core_skill_state::expected_link_target(source, link_parent);
    match fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::read_link(&link) {
            Ok(existing) if existing == Path::new(&target) => {
                report.rows.push(ActionRow {
                    agent: agent.to_string(),
                    skill: name.to_string(),
                    action: Action::Noop,
                    path: link,
                    reason: None,
                });
            }
            Ok(_) => {
                if let Err(error) =
                    fs::remove_file(&link).and_then(|()| create_symlink(Path::new(&target), &link))
                {
                    append_error(&mut report.rows, agent, name, &link, error);
                    return;
                }
                report.rows.push(ActionRow {
                    agent: agent.to_string(),
                    skill: name.to_string(),
                    action: Action::Replaced,
                    path: link,
                    reason: None,
                });
            }
            Err(error) => append_error(&mut report.rows, agent, name, &link, error),
        },
        Ok(_) => report.rows.push(ActionRow {
            agent: agent.to_string(),
            skill: name.to_string(),
            action: Action::Warning,
            path: link,
            reason: Some("user content at target preserved".to_string()),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(error) = create_symlink(Path::new(&target), &link) {
                append_error(&mut report.rows, agent, name, &link, error);
                return;
            }
            report.rows.push(ActionRow {
                agent: agent.to_string(),
                skill: name.to_string(),
                action: Action::Installed,
                path: link,
                reason: None,
            });
        }
        Err(error) => append_error(&mut report.rows, agent, name, &link, error),
    }
}

fn remove_stale_project_links(
    agent: &str,
    link_parent: &Path,
    source_names: &[OsString],
    report: &mut InstallReport,
) {
    if !link_parent.is_dir() {
        return;
    }
    let mut links = match sorted_entries(link_parent) {
        Ok(entries) => entries,
        Err(error) => {
            append_error(&mut report.rows, agent, "", link_parent, error);
            return;
        }
    };
    for link in links.drain(..) {
        let name_os = link.file_name().map(OsString::from).unwrap_or_default();
        if source_names.contains(&name_os) {
            continue;
        }
        let name = name_os.to_string_lossy().to_string();
        match fs::symlink_metadata(&link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if let Err(error) = fs::remove_file(&link) {
                    append_error(&mut report.rows, agent, &name, &link, error);
                    continue;
                }
                report.rows.push(ActionRow {
                    agent: agent.to_string(),
                    skill: name,
                    action: Action::Removed,
                    path: link,
                    reason: Some("stale".to_string()),
                });
            }
            Ok(_) => report.rows.push(ActionRow {
                agent: agent.to_string(),
                skill: name,
                action: Action::Warning,
                path: link,
                reason: Some("user content at stale target preserved".to_string()),
            }),
            Err(error) => append_error(&mut report.rows, agent, &name, &link, error),
        }
    }
}

fn uninstall_project(
    project_root: &Path,
    target: &Path,
    selection: AgentSelection,
) -> Result<InstallReport, String> {
    let sources = solstone_core_skill_state::discover_project_sources(project_root)?;
    let mut report = InstallReport::default();
    for (agent, link_parent) in project_targets(target, selection)? {
        for source in &sources {
            let name = source
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            let link = link_parent.join(name);
            match fs::symlink_metadata(&link) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    report.rows.push(ActionRow {
                        agent: agent.to_string(),
                        skill: name.to_string(),
                        action: Action::Skipped,
                        path: link,
                        reason: Some("nothing to remove".to_string()),
                    });
                }
                Err(error) => append_error(&mut report.rows, agent, name, &link, error),
                Ok(metadata) if !metadata.file_type().is_symlink() => {
                    report.rows.push(ActionRow {
                        agent: agent.to_string(),
                        skill: name.to_string(),
                        action: Action::Error,
                        path: link,
                        reason: Some("refusing to remove non-symlink".to_string()),
                    });
                }
                Ok(_) => {
                    if let Err(error) = fs::remove_file(&link) {
                        append_error(&mut report.rows, agent, name, &link, error);
                        continue;
                    }
                    report.rows.push(ActionRow {
                        agent: agent.to_string(),
                        skill: name.to_string(),
                        action: Action::Removed,
                        path: link,
                        reason: None,
                    });
                }
            }
        }
    }
    Ok(report)
}

fn list_user_status(skill_dir: &Path, home: &Path) -> Vec<StatusRow> {
    USER_AGENT_ORDER
        .into_iter()
        .map(|agent| {
            let spec = agent.spec();
            let target = home.join(spec.skills_dir).join(
                skill_dir
                    .file_name()
                    .unwrap_or_else(|| OsStr::new(USER_SKILL_NAME)),
            );
            let state = if target.join("SKILL.md").is_file() {
                "installed"
            } else {
                "not installed"
            };
            StatusRow {
                agent: spec.name.to_string(),
                skill: USER_SKILL_NAME.to_string(),
                state,
            }
        })
        .collect()
}

fn list_project_status(project_root: &Path, target: &Path) -> Result<Vec<StatusRow>, String> {
    let mut rows = Vec::new();
    for (agent, link_parent) in project_targets(target, AgentSelection::All)? {
        for link in
            solstone_core_skill_state::inspect_router_skill_links(project_root, &link_parent)?
        {
            let state = if link.state == solstone_core_skill_state::RouterSkillLinkState::Installed
            {
                "installed"
            } else {
                "not installed"
            };
            rows.push(StatusRow {
                agent: agent.to_string(),
                skill: link.name,
                state,
            });
        }
    }
    Ok(rows)
}

fn report_output(report: InstallReport, operation: &str) -> CommandOutput {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut warnings = Vec::new();
    for row in &report.rows {
        match row.action {
            Action::Noop => {}
            Action::Warning => warnings.push(row),
            Action::Error => {
                stderr.push_str(&format!(
                    "error: {operation} {}: {}\n",
                    row.path.display(),
                    row.reason.as_deref().unwrap_or_default()
                ));
            }
            Action::Skipped => {
                let skill = if row.skill.is_empty() {
                    String::new()
                } else {
                    format!(" {}", row.skill)
                };
                stdout.push_str(&format!(
                    "skipped {}{} ({})\n",
                    row.agent,
                    skill,
                    row.reason.as_deref().unwrap_or_default()
                ));
            }
            Action::Removed if row.reason.is_some() => {
                stdout.push_str(&format!(
                    "removed {} {} ({}) -> {}\n",
                    row.agent,
                    row.skill,
                    row.reason.as_deref().unwrap_or_default(),
                    row.path.display()
                ));
            }
            Action::Removed => {
                stdout.push_str(&format!(
                    "removed {} {} -> {}\n",
                    row.agent,
                    row.skill,
                    row.path.display()
                ));
            }
            Action::Installed => {
                stdout.push_str(&format!(
                    "installed {} {} -> {}\n",
                    row.agent,
                    row.skill,
                    row.path.display()
                ));
            }
            Action::Replaced => {
                stdout.push_str(&format!(
                    "replaced {} {} -> {}\n",
                    row.agent,
                    row.skill,
                    row.path.display()
                ));
            }
        }
    }
    if !warnings.is_empty() {
        stdout.push_str("Warnings:\n");
        for row in warnings {
            stdout.push_str(&format!(
                "warning {} {} -> {} ({})\n",
                row.agent,
                row.skill,
                row.path.display(),
                row.reason.as_deref().unwrap_or_default()
            ));
        }
    }
    if report.all_skipped() {
        stdout.push_str(GLOBAL_SKIP_MESSAGE);
        stdout.push('\n');
    }
    CommandOutput {
        stdout,
        stderr,
        exit: if report.error_count() > 0 { 1 } else { 0 },
    }
}

fn status_output(rows: &[StatusRow]) -> String {
    let mut output = format!("{:<10} {:<20} state\n", "agent", "skill");
    for row in rows {
        output.push_str(&format!(
            "{:<10} {:<20} {}\n",
            row.agent, row.skill, row.state
        ));
    }
    output
}

fn append_error(
    rows: &mut Vec<ActionRow>,
    agent: &str,
    skill: &str,
    path: &Path,
    error: io::Error,
) {
    rows.push(ActionRow {
        agent: agent.to_string(),
        skill: skill.to_string(),
        action: Action::Error,
        path: path.to_path_buf(),
        reason: Some(error.to_string()),
    });
}

fn tree_matches(src_dir: &Path, dst_dir: &Path) -> io::Result<bool> {
    let src_files = collect_file_rel_paths(src_dir)?;
    let dst_files = collect_file_rel_paths(dst_dir)?;
    if src_files != dst_files {
        return Ok(false);
    }
    for rel in src_files {
        if fs::read(src_dir.join(&rel))? != fs::read(dst_dir.join(&rel))? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_tree_files(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dst_dir)?;
    for rel in collect_file_rel_paths(src_dir)? {
        copy_file_0600(&src_dir.join(&rel), &dst_dir.join(rel))?;
    }
    Ok(())
}

fn collect_file_rel_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    collect_file_rel_paths_inner(root, root, &mut output)?;
    output.sort();
    Ok(output)
}

fn collect_file_rel_paths_inner(
    root: &Path,
    dir: &Path,
    output: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for entry in sorted_entries(dir)? {
        let metadata = fs::symlink_metadata(&entry)?;
        if metadata.file_type().is_dir() {
            collect_file_rel_paths_inner(root, &entry, output)?;
            continue;
        }
        if fs::metadata(&entry).is_ok_and(|metadata| metadata.is_file()) {
            output.push(entry.strip_prefix(root).unwrap_or(&entry).to_path_buf());
        }
    }
    Ok(())
}

fn sorted_entries(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn copy_file_0600(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(dst);
    let result = (|| {
        let mut input = File::open(src)?;
        let mut output = create_temp_file_0600(&temp)?;
        let mut buffer = Vec::new();
        input.read_to_end(&mut buffer)?;
        output.write_all(&buffer)?;
        output.sync_all()?;
        drop(output);
        set_mode_0600(&temp)?;
        fs::rename(&temp, dst)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temp_path(dst: &Path) -> PathBuf {
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(".tmp_{}_{}.tmp", std::process::id(), count);
    dst.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

#[cfg(unix)]
fn create_temp_file_0600(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_temp_file_0600(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlink creation unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::*;

    fn unique_temp(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-core-sol-skills-{name}-"))
            .tempdir()
            .expect("tempdir")
    }

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../core/fixtures/native-sol/skills-parity-v1/vectors.json")
    }

    /// The payload root of this checkout — the directory `sol skills install`
    /// resolves to at runtime, which is what these fixtures are standing in for.
    fn source_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT)
            .canonicalize()
            .expect("source checkout payload root should canonicalize")
    }

    fn subst(text: &str, values: &BTreeMap<String, String>) -> String {
        let mut output = text.to_string();
        let mut replacements = values.iter().collect::<Vec<_>>();
        replacements.sort_by_key(|(token, _value)| std::cmp::Reverse(token.len()));
        for (token, value) in replacements {
            output = output.replace(token, value);
        }
        output
    }

    fn setup_vector(ops: &[Value], values: &BTreeMap<String, String>) {
        for op in ops {
            let kind = op["op"].as_str().expect("setup op should have kind");
            let path = PathBuf::from(subst(
                op["path"].as_str().expect("setup op should have path"),
                values,
            ));
            match kind {
                "mkdir" => fs::create_dir_all(&path).expect("mkdir setup"),
                "write_file" | "mutate_file" => {
                    let content = op["content_b64"]
                        .as_str()
                        .expect("write setup should have content");
                    let bytes = base64_decode(content);
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).expect("write setup parent");
                    }
                    fs::write(&path, bytes).expect("write setup file");
                }
                "symlink" => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).expect("symlink setup parent");
                    }
                    create_symlink(
                        Path::new(&subst(
                            op["target"].as_str().expect("symlink setup target"),
                            values,
                        )),
                        &path,
                    )
                    .expect("symlink setup");
                }
                "remove" => {
                    if path.is_symlink() || path.is_file() {
                        fs::remove_file(&path).expect("remove file setup");
                    } else if path.is_dir() {
                        fs::remove_dir_all(&path).expect("remove dir setup");
                    }
                }
                "copy_user_skill" => {
                    copy_tree_files(&source_root().join("solstone/talent/sol"), &path)
                        .expect("copy user skill setup");
                }
                "project_links" => {
                    let agent = op.get("agent").and_then(Value::as_str).unwrap_or("all");
                    let project = path;
                    let link_parents = if agent == "claude" {
                        vec![project.join(PROJECT_CLAUDE_SKILLS_REL)]
                    } else {
                        vec![
                            project.join(PROJECT_CLAUDE_SKILLS_REL),
                            project.join(PROJECT_AGENTS_SKILLS_REL),
                        ]
                    };
                    for link_parent in link_parents {
                        fs::create_dir_all(&link_parent).expect("project link parent setup");
                        for name in ["journal", "sol"] {
                            let source = source_root().join("solstone/talent").join(name);
                            let target = solstone_core_skill_state::expected_link_target(
                                &source,
                                &link_parent,
                            );
                            create_symlink(Path::new(&target), &link_parent.join(name))
                                .expect("project symlink setup");
                        }
                    }
                }
                other => panic!("unknown setup op {other}"),
            }
        }
    }

    fn base64_decode(input: &str) -> Vec<u8> {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = Vec::new();
        let mut buffer = 0_u32;
        let mut bits = 0_u8;
        for byte in input.bytes() {
            if byte == b'=' {
                break;
            }
            let value = TABLE
                .iter()
                .position(|candidate| *candidate == byte)
                .expect("fixture base64 should be valid") as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            while bits >= 8 {
                bits -= 8;
                output.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        output
    }

    #[test]
    fn skills_parity_vectors_match_native() {
        let text = fs::read_to_string(fixture_root()).expect("read skills parity vectors");
        let fixture: Value = serde_json::from_str(&text).expect("parse skills parity vectors");
        assert_eq!(fixture["schema"], "native-sol-skills-parity-v1");
        let vectors = fixture["vectors"]
            .as_array()
            .expect("vectors should be array");
        assert_eq!(vectors.len(), 30);

        for vector in vectors {
            let id = vector["id"].as_str().expect("vector id");
            let temp = unique_temp(id);
            let root = source_root();
            let fake_root = temp.path().join("fake-root");
            fs::create_dir_all(fake_root.join("solstone/talent")).expect("fake root setup");
            let mut values = BTreeMap::new();
            values.insert("${PROJECT_ROOT}".to_string(), root.display().to_string());
            values.insert(
                "${TEMP_ROOT}".to_string(),
                temp.path().display().to_string(),
            );
            values.insert("${FAKE_ROOT}".to_string(), fake_root.display().to_string());
            let home = PathBuf::from(subst(vector["home"].as_str().expect("home"), &values));
            let cwd = PathBuf::from(subst(vector["cwd"].as_str().expect("cwd"), &values));
            let project_root = PathBuf::from(subst(
                vector["project_root"].as_str().expect("project_root"),
                &values,
            ));
            fs::create_dir_all(&home).expect("create home");
            fs::create_dir_all(&cwd).expect("create cwd");
            values.insert("${HOME}".to_string(), home.display().to_string());
            values.insert("${CWD}".to_string(), cwd.display().to_string());
            setup_vector(
                vector["setup"].as_array().expect("setup should be array"),
                &values,
            );
            let args = vector["argv"]
                .as_array()
                .expect("argv should be array")
                .iter()
                .skip(1)
                .map(|item| OsString::from(subst(item.as_str().expect("argv item"), &values)))
                .collect::<Vec<_>>();
            let output = match parse_command(&args) {
                Ok(command) => run_with_context(
                    command,
                    &RuntimeContext {
                        home: home.clone(),
                        cwd: cwd.clone(),
                        project_root,
                    },
                ),
                Err(message) => usage_error(message),
            };
            let expected = &vector["expected"];
            assert_eq!(
                output.exit,
                expected["exit"].as_i64().unwrap() as i32,
                "{id} exit"
            );
            assert_eq!(
                output.stdout,
                subst(expected["stdout"].as_str().expect("stdout"), &values),
                "{id} stdout"
            );
            if expected
                .get("compare_stderr")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                assert_eq!(
                    output.stderr,
                    subst(expected["stderr"].as_str().expect("stderr"), &values),
                    "{id} stderr"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn installed_user_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let temp = unique_temp("mode");
        let home = temp.path().join("home");
        let context = RuntimeContext {
            home: home.clone(),
            cwd: temp.path().to_path_buf(),
            project_root: source_root(),
        };
        let command = parse_command(&[
            OsString::from("install"),
            OsString::from("--agent"),
            OsString::from("claude"),
        ])
        .expect("parse install");

        let output = run_with_context(command, &context);

        assert_eq!(output.exit, 0);
        let mode = fs::metadata(home.join(".claude/skills/sol/SKILL.md"))
            .expect("installed skill metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn user_reinstall_noop_preserves_inode_and_mtime() {
        use std::os::unix::fs::MetadataExt;
        let temp = unique_temp("user-idempotent");
        let home = temp.path().join("home");
        let context = RuntimeContext {
            home: home.clone(),
            cwd: temp.path().to_path_buf(),
            project_root: source_root(),
        };
        let command = parse_command(&[
            OsString::from("install"),
            OsString::from("--agent"),
            OsString::from("claude"),
        ])
        .expect("parse install");
        assert_eq!(run_with_context(command.clone(), &context).exit, 0);
        let file = home.join(".claude/skills/sol/SKILL.md");
        let before = fs::metadata(&file).expect("before metadata");

        assert_eq!(run_with_context(command, &context).exit, 0);

        let after = fs::metadata(&file).expect("after metadata");
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mtime_nsec(), after.mtime_nsec());
        assert_eq!(before.mtime(), after.mtime());
    }

    #[cfg(unix)]
    #[test]
    fn project_reinstall_noop_preserves_readlink_and_lstat_mtime() {
        use std::os::unix::fs::MetadataExt;
        let temp = unique_temp("project-idempotent");
        let project = temp.path().join("project");
        let context = RuntimeContext {
            home: temp.path().join("home"),
            cwd: temp.path().to_path_buf(),
            project_root: source_root(),
        };
        let command = parse_command(&[
            OsString::from("install"),
            OsString::from("--project"),
            OsString::from(project.as_os_str()),
            OsString::from("--agent"),
            OsString::from("all"),
        ])
        .expect("parse project install");
        assert_eq!(run_with_context(command.clone(), &context).exit, 0);
        let link = project.join(".claude/skills/journal");
        let before_link = fs::read_link(&link).expect("before readlink");
        let before_meta = fs::symlink_metadata(&link).expect("before lstat");

        assert_eq!(run_with_context(command, &context).exit, 0);

        let after_link = fs::read_link(&link).expect("after readlink");
        let after_meta = fs::symlink_metadata(&link).expect("after lstat");
        assert_eq!(before_link, after_link);
        assert_eq!(before_meta.mtime(), after_meta.mtime());
        assert_eq!(before_meta.mtime_nsec(), after_meta.mtime_nsec());
    }

    #[cfg(unix)]
    #[test]
    fn user_install_replaces_symlink_without_touching_pointed_to_directory() {
        let temp = unique_temp("symlink-replace");
        let home = temp.path().join("home");
        let external = temp.path().join("external");
        fs::create_dir_all(&external).expect("external dir");
        fs::write(external.join("keep.txt"), "keep\n").expect("external content");
        let link = home.join(".claude/skills/sol");
        fs::create_dir_all(link.parent().unwrap()).expect("link parent");
        create_symlink(&external, &link).expect("setup symlink");
        let context = RuntimeContext {
            home: home.clone(),
            cwd: temp.path().to_path_buf(),
            project_root: source_root(),
        };
        let command = parse_command(&[
            OsString::from("install"),
            OsString::from("--agent"),
            OsString::from("claude"),
        ])
        .expect("parse install");

        let output = run_with_context(command, &context);

        assert_eq!(output.exit, 0);
        assert_eq!(
            fs::read_to_string(external.join("keep.txt")).unwrap(),
            "keep\n"
        );
        assert!(
            !fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(link.join("SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn skills_canonicalizes_symlinked_checkout_root_for_project_links() {
        let temp = unique_temp("symlinked-root");
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir_all(real.join("solstone/talent")).expect("real talent");
        fs::create_dir_all(real.join("solstone/talent/sol")).expect("real sol");
        fs::create_dir_all(real.join("solstone/talent/journal")).expect("real journal");
        fs::write(real.join("solstone/talent/sol/SKILL.md"), "---\n").expect("sol skill");
        fs::write(real.join("solstone/talent/journal/SKILL.md"), "---\n").expect("journal skill");
        create_symlink(&real, &linked).expect("linked checkout");
        let project = temp.path().join("project");
        let context = RuntimeContext {
            home: temp.path().join("home"),
            cwd: temp.path().to_path_buf(),
            project_root: canonicalize_project_root_for_skills(linked.clone()),
        };
        let command = parse_command(&[
            OsString::from("install"),
            OsString::from("--project"),
            OsString::from(project.as_os_str()),
            OsString::from("--agent"),
            OsString::from("claude"),
        ])
        .expect("parse install");

        let output = run_with_context(command, &context);

        assert_eq!(output.exit, 0);
        let link_target =
            fs::read_link(project.join(".claude/skills/journal")).expect("project journal link");
        let link_target = link_target.to_string_lossy();
        assert!(link_target.contains("real/solstone/talent/journal"));
        assert!(!link_target.contains("linked"));
    }

    #[test]
    fn row_order_matches_python_contract() {
        let temp = unique_temp("row-order");
        let home = temp.path().join("home");
        let context = RuntimeContext {
            home: home.clone(),
            cwd: temp.path().to_path_buf(),
            project_root: source_root(),
        };
        let user = run_with_context(
            parse_command(&[OsString::from("install")]).expect("parse user install"),
            &context,
        );
        assert!(user.stdout.find("claude").unwrap() < user.stdout.find("codex").unwrap());
        assert!(user.stdout.find("codex").unwrap() < user.stdout.find("gemini").unwrap());
        let project = temp.path().join("project");
        let output = run_with_context(
            parse_command(&[
                OsString::from("install"),
                OsString::from("--project"),
                OsString::from(project.as_os_str()),
                OsString::from("--agent"),
                OsString::from("claude"),
            ])
            .expect("parse project install"),
            &context,
        );
        assert!(output.stdout.find("journal").unwrap() < output.stdout.find(" sol ").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn mixed_writability_regular_file_component_still_installs_writable_agent() {
        let temp = unique_temp("mixed-writability-file-component");
        let home = temp.path().join("home");
        let claude_config = home.join(".claude");
        let claude_skills_root = home.join(".claude/skills");
        let codex_skills_root = home.join(".codex/skills");
        fs::create_dir_all(&home).expect("create home");
        fs::write(&claude_config, "not a directory").expect("create regular file component");
        fs::create_dir_all(&codex_skills_root).expect("create writable codex skills root");
        let skill_dir = source_root().join("solstone/talent/sol");

        let report = install_user(&skill_dir, &home, AgentSelection::All);

        let error = report
            .rows
            .iter()
            .find(|row| row.action == Action::Error)
            .expect("expected one structural error row");
        assert_eq!(error.agent, "claude");
        assert_eq!(error.path, claude_skills_root);
        let output = report_output(report, "install");
        assert_eq!(output.exit, 1);
        assert!(home.join(".codex/skills/sol/SKILL.md").is_file());
    }
}
