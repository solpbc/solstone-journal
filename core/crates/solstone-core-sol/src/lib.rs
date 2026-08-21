// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Read, Result as IoResult, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs};

use chrono::Local;
use serde_json::json;
use solstone_core_sol_client::command::{CommandContext, CommandOutput};
use solstone_core_sol_client::resident::{ResidentHandler, ShutdownSignal};
use solstone_core_sol_client::seam::{
    BuildIdentityProvider, ClientItemIdProvider, Clock, FileProvider, HttpTransport,
    LinkJoinPairingSeam, LinkServeRunner, ProcessOutput, ProcessSpawner,
};
#[cfg(target_os = "ios")]
use solstone_core_sol_client::seam::{
    LinkJoinDirectRequest, LinkJoinPairingError, LinkJoinPairingErrorKind, LinkJoinRelayRequest,
    LinkServeError, LinkServeErrorKind,
};
use solstone_core_sol_client::transport::UreqHttpTransport;
use solstone_core_sol_client_cli::{
    DispatchSeams, LinkDispatch, LinkDispatchSeams, Outcome, dispatch_sol_call_with_seams,
    dispatch_sol_import_with_seams, dispatch_sol_link_with_seams, dispatch_sol_status_with_seams,
    evaluate_args, help,
};
#[cfg(not(target_os = "ios"))]
use solstone_core_sol_link::{SplLinkJoinPairingSeam, SplLinkServeRunner};

mod skills;

pub use solstone_core_cli_boundary::{JOURNAL_HOST_COMMAND_COUNT, JOURNAL_HOST_COMMANDS};

const EXIT_USAGE: u8 = 64;
const EXIT_CONFIG: u8 = 78;
const EXIT_TEMPFAIL: u8 = 75;
const DEFAULT_CONVEY_PORT: i64 = 5015;
const SOLSTONE_CONVEY_PORT_ENV: &str = "SOLSTONE_CONVEY_PORT";
const USAGE: &str = "Usage: solstone <command> [args...]\n";
const SERVICE_MOVED_EXIT: i32 = 2;
const SOL_SERVICE_CMD_REMOVED_ERROR_TAIL: &str = "('solstone' is the journal-access surface; 'journal' surfaces journal-service commands; see 'journal --help'.)";

fn resolve_convey_port(env: &BTreeMap<String, String>) -> Result<i64, CommandOutput> {
    match env.get(SOLSTONE_CONVEY_PORT_ENV) {
        None => Ok(DEFAULT_CONVEY_PORT),
        Some(value) => match value.parse::<i64>() {
            Ok(port) if (1..=65535).contains(&port) => Ok(port),
            _ => Err(CommandOutput::failure(
                "SOLSTONE_CONVEY_PORT must be an integer from 1 to 65535.\n",
                i32::from(EXIT_CONFIG),
            )),
        },
    }
}

pub fn run(public_argv0: &str, args: Vec<OsString>) -> ExitCode {
    run_with_stdin_provider(public_argv0, args, &RealStdinProvider)
}

fn run_with_stdin_provider(
    _public_argv0: &str,
    args: Vec<OsString>,
    stdin_provider: &dyn StdinProvider,
) -> ExitCode {
    let mut args = args;
    if args.first().is_some_and(|arg| is_verbose_flag(arg)) {
        args.remove(0);
    }
    match args.as_slice() {
        [] => render_output(help_output()),
        [flag] if flag == OsStr::new("--version") || flag == OsStr::new("-V") => {
            render_output(version_output())
        }
        [command]
            if command == OsStr::new("--help")
                || command == OsStr::new("-h")
                || command == OsStr::new("help") =>
        {
            render_output(help_output())
        }
        [command] if command == OsStr::new("root") => run_root(),
        [command, rest @ ..] if command == OsStr::new("status") => {
            run_top_level_native(&args, "status", rest, stdin_provider)
        }
        [command, rest @ ..] if command == OsStr::new("skills") => render_output(skills::run(rest)),
        [command, rest @ ..] if command == OsStr::new("call") => {
            run_call(&args, rest, stdin_provider)
        }
        [command, rest @ ..] if command == OsStr::new("import") => {
            run_top_level_native(&args, "import", rest, stdin_provider)
        }
        [command, rest @ ..] if command == OsStr::new("link") => {
            run_top_level_link(&args, rest, stdin_provider)
        }
        [flag, ..] if flag.to_string_lossy().starts_with('-') => {
            render_output(usage_error_output())
        }
        [command, ..] if is_journal_host_command(command) => {
            render_output(service_moved_output(command))
        }
        _ => render_output(unsupported_output()),
    }
}

fn version_output() -> CommandOutput {
    CommandOutput::success(format!("solstone {}\n", env!("CARGO_PKG_VERSION")))
}

fn help_output() -> CommandOutput {
    CommandOutput {
        stdout: help::render_root_help(),
        stderr: String::new(),
        exit: 0,
    }
}

fn usage_error_output() -> CommandOutput {
    CommandOutput::failure(USAGE, i32::from(EXIT_USAGE))
}

fn unsupported_output() -> CommandOutput {
    CommandOutput::failure(
        "Unsupported native solstone command.\n",
        i32::from(EXIT_USAGE),
    )
}

fn service_moved_output(command: &OsStr) -> CommandOutput {
    let command = command.to_string_lossy();
    CommandOutput::failure(
        format!(
            "'{command}' moved to 'journal {command}' — run that instead.\n{SOL_SERVICE_CMD_REMOVED_ERROR_TAIL}\n"
        ),
        SERVICE_MOVED_EXIT,
    )
}

fn is_journal_host_command(command: &OsStr) -> bool {
    debug_assert_eq!(JOURNAL_HOST_COMMANDS.len(), JOURNAL_HOST_COMMAND_COUNT);
    command
        .to_str()
        .is_some_and(|value| JOURNAL_HOST_COMMANDS.binary_search(&value).is_ok())
}

fn is_verbose_flag(command: &OsStr) -> bool {
    command == OsStr::new("-v") || command == OsStr::new("--verbose")
}

#[derive(Debug)]
enum ProjectRootError {
    CurrentExe(std::io::Error),
    Unclassified(PathBuf),
}

impl std::fmt::Display for ProjectRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectRootError::CurrentExe(error) => write!(
                formatter,
                "native solstone project root resolution failed: could not inspect current executable: {error}"
            ),
            ProjectRootError::Unclassified(executable) => write!(
                formatter,
                "native solstone project root resolution failed: could not locate source checkout or installed solstone package from {}",
                executable.display()
            ),
        }
    }
}

impl std::error::Error for ProjectRootError {}

fn run_root() -> ExitCode {
    match resolve_project_root() {
        Ok(root) => render_output(CommandOutput::success(format!("{}\n", root.display()))),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_CONFIG)
        }
    }
}

fn resolve_project_root() -> Result<PathBuf, ProjectRootError> {
    let executable = env::current_exe().map_err(ProjectRootError::CurrentExe)?;
    resolve_project_root_from_executable(&executable)
}

fn resolve_project_root_from_executable(executable: &Path) -> Result<PathBuf, ProjectRootError> {
    let Some(executable_dir) = executable.parent() else {
        return Err(ProjectRootError::Unclassified(executable.to_path_buf()));
    };
    solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)
        .ok_or_else(|| ProjectRootError::Unclassified(executable.to_path_buf()))
}

fn run_call(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
) -> ExitCode {
    let Some(args) = os_strings_to_strings(command_args) else {
        return render_output(usage_error_output());
    };
    if let Some(output) = help::render_sol_call_help(&args) {
        return render_output(output);
    }
    run_dispatched(all_args, command_args, stdin_provider)
}

fn run_top_level_native(
    all_args: &[OsString],
    command: &str,
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
) -> ExitCode {
    let Some(args) = os_strings_to_strings(command_args) else {
        return render_output(usage_error_output());
    };
    if let Some(output) = help::render_top_level_help(command, &args) {
        return render_output(output);
    }
    run_dispatched(all_args, command_args, stdin_provider)
}

fn run_top_level_link(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
) -> ExitCode {
    run_top_level_link_with_env_provider(all_args, command_args, stdin_provider, &|| {
        env::vars().collect::<BTreeMap<_, _>>()
    })
}

fn run_top_level_link_with_env_provider(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
    env_provider: &dyn Fn() -> BTreeMap<String, String>,
) -> ExitCode {
    let args = match os_strings_to_strings(command_args) {
        Some(args) => args,
        None => return render_output(usage_error_output()),
    };
    if let Some(output) = help::render_link_help(&args) {
        return render_output(output);
    }
    let env = env_provider();
    run_top_level_link_with_env(all_args, command_args, stdin_provider, &env)
}

fn run_top_level_link_with_env(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
    env: &BTreeMap<String, String>,
) -> ExitCode {
    run_top_level_link_with_runtime(
        all_args,
        command_args,
        stdin_provider,
        env,
        &|port| Box::new(UreqHttpTransport::new(port)),
        link_join_pairing_seam(),
        link_serve_runner(),
    )
}

fn run_top_level_link_with_runtime(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
    env: &BTreeMap<String, String>,
    transport_factory: &dyn Fn(i64) -> Box<dyn HttpTransport>,
    link_pairing: &dyn LinkJoinPairingSeam,
    link_serve: &dyn LinkServeRunner,
) -> ExitCode {
    let args = match os_strings_to_strings(command_args) {
        Some(args) => args,
        None => return render_output(usage_error_output()),
    };
    if let Some(output) = help::render_link_help(&args) {
        return render_output(output);
    }
    let dispatch_args = match os_strings_to_strings(all_args) {
        Some(args) => args,
        None => return render_output(usage_error_output()),
    };
    let today = Local::now().format("%Y%m%d").to_string();
    let port = match resolve_convey_port(env) {
        Ok(port) => port,
        Err(output) => return render_output(output),
    };
    let transport = transport_factory(port);
    let stdin = match stdin_provider.read_if_piped() {
        Ok(Some(value)) => value,
        Ok(None) => String::new(),
        Err(error) => {
            eprintln!("native solstone stdin read failed: {error}");
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let clock = SystemClock::default();
    let files = RealFileProvider;
    let dispatch = dispatch_top_level_link_with_runtime_seams(
        &dispatch_args,
        TopLevelLinkRuntime {
            env,
            stdin: &stdin,
            today: &today,
            transport: transport.as_ref(),
            clock: &clock,
            files: &files,
            link_pairing,
            link_serve,
        },
    );
    match dispatch {
        LinkDispatch::Buffered(output) => render_output(output),
        LinkDispatch::Resident {
            handler,
            args: resident_args,
        } => {
            let context = CommandContext {
                args: &resident_args,
                env,
                stdin: &stdin,
                today: &today,
                transport: transport.as_ref(),
                clock: Some(&clock),
                files: Some(&files),
                build_identity: None,
                client_item_ids: None,
                notification_sink: None,
                link_pairing: Some(link_pairing),
                link_serve: Some(link_serve),
            };
            run_resident_command(handler, context)
        }
    }
}

struct TopLevelLinkRuntime<'a> {
    env: &'a BTreeMap<String, String>,
    stdin: &'a str,
    today: &'a str,
    transport: &'a dyn HttpTransport,
    clock: &'a dyn Clock,
    files: &'a dyn FileProvider,
    link_pairing: &'a dyn LinkJoinPairingSeam,
    link_serve: &'a dyn LinkServeRunner,
}

fn dispatch_top_level_link_with_runtime_seams(
    args: &[String],
    runtime: TopLevelLinkRuntime<'_>,
) -> LinkDispatch {
    dispatch_sol_link_with_seams(
        args,
        runtime.env,
        runtime.stdin,
        runtime.today,
        LinkDispatchSeams {
            transport: runtime.transport,
            clock: Some(runtime.clock),
            files: Some(runtime.files),
            link_pairing: Some(runtime.link_pairing),
            link_serve: Some(runtime.link_serve),
        },
    )
}

fn run_dispatched(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
) -> ExitCode {
    let env = env::vars().collect::<BTreeMap<_, _>>();
    run_dispatched_with_env(all_args, command_args, stdin_provider, &env)
}

fn run_dispatched_with_env(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
    env: &BTreeMap<String, String>,
) -> ExitCode {
    run_dispatched_with_runtime(all_args, command_args, stdin_provider, env, &|port| {
        Box::new(UreqHttpTransport::new(port))
    })
}

fn run_dispatched_with_runtime(
    all_args: &[OsString],
    command_args: &[OsString],
    stdin_provider: &dyn StdinProvider,
    env: &BTreeMap<String, String>,
    transport_factory: &dyn Fn(i64) -> Box<dyn HttpTransport>,
) -> ExitCode {
    let outcome = evaluate_args(all_args);
    if matches!(outcome, Outcome::Unsupported { .. }) {
        return render_output(unsupported_output());
    }
    let today = Local::now().format("%Y%m%d").to_string();
    let args = match os_strings_to_strings(command_args) {
        Some(args) => args,
        None => {
            eprint!("{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let port = match resolve_convey_port(env) {
        Ok(port) => port,
        Err(output) => return render_output(output),
    };
    let transport = transport_factory(port);
    let stdin = match stdin_provider.read_if_piped() {
        Ok(Some(value)) => value,
        Ok(None) => String::new(),
        Err(error) => {
            eprintln!("native solstone stdin read failed: {error}");
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let files = RealFileProvider;
    let build_identity = RealBuildIdentityProvider;
    let client_item_ids = RealClientItemIdProvider;
    let output = match outcome {
        Outcome::Migrated { .. } | Outcome::MovedStub { .. } => dispatch_sol_call_with_seams(
            &args,
            env,
            &stdin,
            &today,
            DispatchSeams {
                transport: transport.as_ref(),
                clock: None,
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        ),
        Outcome::Import { .. } => dispatch_sol_import_with_seams(
            &args,
            env,
            &stdin,
            &today,
            DispatchSeams {
                transport: transport.as_ref(),
                clock: None,
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        ),
        Outcome::Status { .. } => dispatch_sol_status_with_seams(
            &args,
            env,
            &stdin,
            &today,
            DispatchSeams {
                transport: transport.as_ref(),
                clock: None,
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        ),
        Outcome::Unsupported { .. } => unsupported_output(),
    };
    render_output(output)
}

#[cfg(not(target_os = "ios"))]
static LINK_JOIN_PAIRING_SEAM: SplLinkJoinPairingSeam = SplLinkJoinPairingSeam;
#[cfg(not(target_os = "ios"))]
static LINK_SERVE_RUNNER: SplLinkServeRunner = SplLinkServeRunner;
#[cfg(target_os = "ios")]
static LINK_JOIN_PAIRING_SEAM: UnavailableLinkJoinPairingSeam = UnavailableLinkJoinPairingSeam;
#[cfg(target_os = "ios")]
static LINK_SERVE_RUNNER: UnavailableLinkServeRunner = UnavailableLinkServeRunner;

fn link_join_pairing_seam() -> &'static dyn LinkJoinPairingSeam {
    &LINK_JOIN_PAIRING_SEAM
}

fn link_serve_runner() -> &'static dyn LinkServeRunner {
    &LINK_SERVE_RUNNER
}

#[cfg(target_os = "ios")]
#[derive(Debug, Default)]
struct UnavailableLinkJoinPairingSeam;

#[cfg(target_os = "ios")]
impl LinkJoinPairingSeam for UnavailableLinkJoinPairingSeam {
    fn pair_direct(
        &self,
        _request: LinkJoinDirectRequest,
    ) -> Result<solstone_core_sol_client::seam::LinkJoinCredential, LinkJoinPairingError> {
        Err(LinkJoinPairingError::new(
            LinkJoinPairingErrorKind::RuntimeUnavailable,
        ))
    }

    fn pair_relay(
        &self,
        _request: LinkJoinRelayRequest,
    ) -> Result<solstone_core_sol_client::seam::LinkJoinCredential, LinkJoinPairingError> {
        Err(LinkJoinPairingError::new(
            LinkJoinPairingErrorKind::RuntimeUnavailable,
        ))
    }
}

#[cfg(target_os = "ios")]
#[derive(Debug, Default)]
struct UnavailableLinkServeRunner;

#[cfg(target_os = "ios")]
impl LinkServeRunner for UnavailableLinkServeRunner {
    fn start(
        &self,
        _request: solstone_core_sol_client::seam::LinkServeRequest,
    ) -> Result<Box<dyn solstone_core_sol_client::seam::LinkServeSession>, LinkServeError> {
        Err(LinkServeError::new(LinkServeErrorKind::RuntimeUnavailable))
    }
}

pub fn run_resident_command(handler: ResidentHandler, context: CommandContext<'_>) -> ExitCode {
    let shutdown = match RealShutdownSignal::install() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            return render_output(CommandOutput::failure(
                format!("native solstone resident signal setup failed: {error}\n"),
                i32::from(EXIT_TEMPFAIL),
            ));
        }
    };

    let resident = match handler(context) {
        Ok(resident) => resident,
        Err(output) => return render_output(output),
    };

    print!("{}", resident.startup());
    if let Err(error) = io::stdout().flush() {
        return render_output(CommandOutput::failure(
            format!("native solstone resident startup flush failed: {error}\n"),
            i32::from(EXIT_TEMPFAIL),
        ));
    }

    let output = resident.serve(&shutdown);
    render_output(output)
}

#[cfg(unix)]
struct RealShutdownSignal {
    signals: nix::sys::signal::SigSet,
}

#[cfg(unix)]
impl RealShutdownSignal {
    fn install() -> Result<Self, nix::errno::Errno> {
        use nix::sys::signal::{SigSet, Signal};

        let mut signals = SigSet::empty();
        signals.add(Signal::SIGINT);
        signals.add(Signal::SIGTERM);
        signals.thread_block()?;
        Ok(Self { signals })
    }
}

#[cfg(unix)]
impl ShutdownSignal for RealShutdownSignal {
    fn wait(&self) {
        self.signals
            .wait()
            .expect("sigwait on a blocked SIGINT/SIGTERM set cannot fail");
    }
}

#[cfg(not(unix))]
struct RealShutdownSignal;

#[cfg(not(unix))]
impl RealShutdownSignal {
    fn install() -> Result<Self, &'static str> {
        Err("shutdown signals are unavailable on this platform")
    }
}

#[cfg(not(unix))]
impl ShutdownSignal for RealShutdownSignal {
    fn wait(&self) {}
}

fn render_output(output: CommandOutput) -> ExitCode {
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    let exit = u8::try_from(output.exit).unwrap_or(EXIT_TEMPFAIL);
    ExitCode::from(exit)
}

trait StdinProvider {
    fn read_if_piped(&self) -> IoResult<Option<String>>;
}

struct RealStdinProvider;

impl StdinProvider for RealStdinProvider {
    fn read_if_piped(&self) -> IoResult<Option<String>> {
        let mut stdin = std::io::stdin();
        if stdin.is_terminal() {
            return Ok(None);
        }
        let mut input = String::new();
        stdin.read_to_string(&mut input)?;
        Ok(Some(input))
    }
}

fn os_strings_to_strings(args: &[OsString]) -> Option<Vec<String>> {
    args.iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect()
}

#[derive(Debug)]
struct SystemClock {
    started: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn monotonic(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[derive(Debug)]
struct RealFileProvider;

impl FileProvider for RealFileProvider {
    fn read(&self, path: &Path) -> IoResult<Vec<u8>> {
        fs::read(path)
    }

    fn read_to_string(&self, path: &Path) -> IoResult<String> {
        fs::read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn canonicalize(&self, path: &Path) -> IoResult<PathBuf> {
        fs::canonicalize(path)
    }
}

struct RealClientItemIdProvider;

impl ClientItemIdProvider for RealClientItemIdProvider {
    fn client_item_id(&self) -> String {
        let mut bytes = [0_u8; 16];
        let read = fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .is_ok();
        if !read {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            bytes.copy_from_slice(&nanos.to_be_bytes());
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

#[derive(Debug)]
struct RealProcessSpawner;

impl ProcessSpawner for RealProcessSpawner {
    fn run(&self, program: &str, args: &[String]) -> IoResult<ProcessOutput> {
        let output = Command::new(program).args(args).output()?;
        Ok(ProcessOutput {
            status: output.status.code().unwrap_or(1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[derive(Debug)]
struct RealBuildIdentityProvider;

impl BuildIdentityProvider for RealBuildIdentityProvider {
    fn build_identity(&self, _journal: &Path) -> Option<serde_json::Value> {
        let spawner = RealProcessSpawner;
        let revision = spawner
            .run(
                "git",
                &[
                    "rev-parse".to_string(),
                    "--short".to_string(),
                    "HEAD".to_string(),
                ],
            )
            .ok()
            .filter(|output| output.status == 0)
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Some(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "revision": revision,
            "platform": {
                "system": env::consts::OS,
                "release": "",
                "machine": env::consts::ARCH,
                "python": "?"
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLI_BOUNDARY_JSON: &str =
        include_str!("../../../fixtures/native-sol/cli-boundary-v1.json");

    fn cli_boundary_errors(value: &serde_json::Value) -> Vec<String> {
        let mut errors = Vec::new();
        let binary_count = value
            .get("binary_count")
            .and_then(serde_json::Value::as_u64);
        if binary_count != Some(1) {
            errors.push("binary_count must be one".to_owned());
        }
        let identities = value
            .get("identities")
            .and_then(serde_json::Value::as_object);
        let Some(identities) = identities else {
            errors.push("identities must be an object".to_owned());
            return errors;
        };
        let identity_names = identities
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if identity_names != string_values(&["journal", "solstone"]) {
            errors.push("identities must contain exactly journal and solstone".to_owned());
        }
        let Some(solstone) = identities.get("solstone") else {
            errors.push("solstone identity is missing".to_owned());
            return errors;
        };
        if solstone
            .get("journal_reach")
            .and_then(serde_json::Value::as_str)
            != Some("api-only")
        {
            errors.push("solstone journal reach must be api-only".to_owned());
        }
        let api = string_set(solstone, "api_commands", &mut errors);
        if api != string_values(&["call", "import", "status"]) {
            errors.push("solstone API command boundary drifted".to_owned());
        }
        let local = string_set(solstone, "invoking_device_commands", &mut errors);
        if local != string_values(&["link", "root", "skills"]) {
            errors.push("solstone invoking-device command boundary drifted".to_owned());
        }
        for duplicate in api.intersection(&local) {
            errors.push(format!("solstone command {duplicate} has two reaches"));
        }
        let forbidden_sol = string_set(solstone, "forbidden_direct_journal_commands", &mut errors);
        if forbidden_sol != string_values(&["--path", "check", "doctor", "notify", "path"]) {
            errors.push("solstone forbidden direct-journal command boundary drifted".to_owned());
        }
        let http_journal_calls = string_set(solstone, "http_paths", &mut errors);
        if http_journal_calls
            != string_values(&[
                "call journal agents",
                "call journal facet create",
                "call journal facet delete",
                "call journal facet mute",
                "call journal facet rename",
                "call journal facet show",
                "call journal facet unmute",
                "call journal facet update",
                "call journal facets",
                "call journal import",
                "call journal imports",
                "call journal news",
                "call journal read",
                "call journal retention config",
                "call journal retention list",
                "call journal search",
                "call journal storage-summary",
            ])
        {
            errors.push("solstone HTTP journal-call boundary drifted".to_owned());
        }
        let http_bindings = http_binding_map(solstone, &mut errors);
        let expected_http_bindings = binding_values(&[
            ("status", &["GET /app/network/api/status"]),
            ("call journal agents", &["GET /app/search/api/agents"]),
            (
                "call journal facet create",
                &["POST /app/settings/api/facet"],
            ),
            (
                "call journal facet delete",
                &["DELETE /app/settings/api/facet/{facet_name}"],
            ),
            (
                "call journal facet mute",
                &["PUT /app/settings/api/facet/{facet_name}"],
            ),
            (
                "call journal facet rename",
                &["POST /app/settings/api/facet/{facet_name}/rename"],
            ),
            (
                "call journal facet show",
                &["GET /app/settings/api/facet/{facet_name}"],
            ),
            (
                "call journal facet unmute",
                &["PUT /app/settings/api/facet/{facet_name}"],
            ),
            (
                "call journal facet update",
                &["PUT /app/settings/api/facet/{facet_name}"],
            ),
            ("call journal facets", &["GET /app/settings/api/facets"]),
            ("call journal import", &["GET /app/import/api/{timestamp}"]),
            ("call journal imports", &["GET /app/import/api/list"]),
            ("call journal news", &["GET /app/news/api/facet/{facet}"]),
            ("call journal read", &["GET /app/search/api/read"]),
            (
                "call journal retention config",
                &[
                    "GET /app/settings/api/storage",
                    "PUT /app/settings/api/storage",
                ],
            ),
            (
                "call journal retention list",
                &["POST /app/settings/api/storage/list"],
            ),
            ("call journal search", &["GET /app/search/api/search"]),
            (
                "call journal storage-summary",
                &["GET /app/settings/api/storage"],
            ),
        ]);
        if http_bindings != expected_http_bindings {
            errors.push("solstone HTTP method/route boundary drifted".to_owned());
        }
        let retired_sol = string_set(solstone, "retired_invocations", &mut errors);
        if retired_sol
            != string_values(&[
                "--path",
                "call journal export",
                "call journal facet doctor",
                "call journal facet merge",
                "call journal merge",
                "call journal news --write",
                "call journal retention purge",
                "check",
                "doctor",
                "notify",
                "path",
            ])
        {
            errors.push("retired solstone invocation census drifted".to_owned());
        }
        let Some(journal) = identities.get("journal") else {
            errors.push("journal identity is missing".to_owned());
            return errors;
        };
        if journal
            .get("journal_reach")
            .and_then(serde_json::Value::as_str)
            != Some("same-device")
        {
            errors.push("journal reach must be same-device".to_owned());
        }
        let may_use = string_set(journal, "may_use", &mut errors);
        if may_use != string_values(&["direct", "localhost-api", "service-process"]) {
            errors.push("journal allowed reach boundary drifted".to_owned());
        }
        let root = string_set(journal, "root_commands", &mut errors);
        if root
            != string_values(&[
                "--path", "check", "contract", "doctor", "indexer", "notify", "path", "root",
                "status",
            ])
        {
            errors.push("journal root command boundary drifted".to_owned());
        }
        let planned_local = string_set(journal, "planned_local_paths", &mut errors);
        if planned_local
            != string_values(&[
                "archive export",
                "archive merge",
                "facet doctor",
                "facet merge",
                "news write",
            ])
        {
            errors.push("journal planned local command boundary drifted".to_owned());
        }
        let service = string_set(journal, "service_commands", &mut errors);
        let expected = JOURNAL_HOST_COMMANDS
            .iter()
            .map(|command| (*command).to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        if service != expected {
            errors.push("journal service command census drifted".to_owned());
        }
        let forbidden = string_set(journal, "forbidden_grammar", &mut errors);
        if !forbidden.contains("dotted-module") {
            errors.push("journal dotted-module grammar must be forbidden".to_owned());
        }
        let legacy = string_set(value, "legacy_exceptions", &mut errors);
        if legacy
            != string_values(&[
                "journal-dotted-module-dispatch",
                "journal-python-public-entrypoint",
            ])
        {
            errors.push("legacy exception census drifted".to_owned());
        }
        errors
    }

    fn string_values(values: &[&str]) -> std::collections::BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn binding_values(
        values: &[(&str, &[&str])],
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        values
            .iter()
            .map(|(path, bindings)| ((*path).to_owned(), string_values(bindings)))
            .collect()
    }

    fn http_binding_map(
        sol: &serde_json::Value,
        errors: &mut Vec<String>,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let Some(entries) = sol
            .get("http_bindings")
            .and_then(serde_json::Value::as_array)
        else {
            errors.push("http_bindings must be an array".to_owned());
            return std::collections::BTreeMap::new();
        };
        let mut output = std::collections::BTreeMap::new();
        for entry in entries {
            let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
                errors.push("http_bindings entry has no string path".to_owned());
                continue;
            };
            let Some(bindings) = entry.get("bindings").and_then(serde_json::Value::as_array) else {
                errors.push(format!("http_bindings {path} has no bindings array"));
                continue;
            };
            let mut projected = std::collections::BTreeSet::new();
            for binding in bindings {
                let method = binding.get("method").and_then(serde_json::Value::as_str);
                let route = binding.get("route").and_then(serde_json::Value::as_str);
                let (Some(method), Some(route)) = (method, route) else {
                    errors.push(format!("http_bindings {path} has a malformed binding"));
                    continue;
                };
                if !projected.insert(format!("{method} {route}")) {
                    errors.push(format!("http_bindings {path} contains a duplicate binding"));
                }
            }
            if projected.is_empty() {
                errors.push(format!("http_bindings {path} must not be empty"));
            }
            if output.insert(path.to_owned(), projected).is_some() {
                errors.push(format!("http_bindings contains duplicate path {path}"));
            }
        }
        output
    }

    fn string_set(
        value: &serde_json::Value,
        field: &str,
        errors: &mut Vec<String>,
    ) -> std::collections::BTreeSet<String> {
        let Some(items) = value.get(field).and_then(serde_json::Value::as_array) else {
            errors.push(format!("{field} must be an array"));
            return std::collections::BTreeSet::new();
        };
        let mut set = std::collections::BTreeSet::new();
        for item in items {
            let Some(item) = item.as_str() else {
                errors.push(format!("{field} contains a non-string"));
                continue;
            };
            if !set.insert(item.to_owned()) {
                errors.push(format!("{field} contains duplicate {item}"));
            }
        }
        set
    }

    #[test]
    fn cli_boundary_fixture_is_total_for_the_current_journal_host_census() {
        let value: serde_json::Value =
            serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture");
        assert_eq!(value["schema"], "solstone-cli-boundary-v1");
        assert_eq!(JOURNAL_HOST_COMMAND_COUNT, 40);
        assert_eq!(cli_boundary_errors(&value), Vec::<String>::new());
    }

    #[test]
    fn cli_boundary_fixture_rejects_two_reaches_and_a_missing_identity() {
        let mut value: serde_json::Value =
            serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture");
        value["identities"]["solstone"]["invoking_device_commands"] =
            serde_json::json!(["link", "status"]);
        value["identities"]["bridge"] = serde_json::json!({});
        let errors = cli_boundary_errors(&value);
        assert!(
            errors
                .iter()
                .any(|error| error == "solstone command status has two reaches")
        );
        assert!(
            errors
                .iter()
                .any(|error| error == "identities must contain exactly journal and solstone")
        );
    }

    use solstone_core_sol_client::seam::{
        ExpectedLinkServeCall, ExpectedLinkServeSession, LinkServeBundle, LinkServeCarrierPolicy,
        LinkServeRequest, ScriptedHttpTransport, ScriptedLinkJoinPairingSeam,
        ScriptedLinkServeRunner,
    };

    struct PanicStdinProvider;

    impl StdinProvider for PanicStdinProvider {
        fn read_if_piped(&self) -> IoResult<Option<String>> {
            panic!("stdin must not be read for this route")
        }
    }

    struct FailingStdinProvider;

    impl StdinProvider for FailingStdinProvider {
        fn read_if_piped(&self) -> IoResult<Option<String>> {
            Err(io::Error::other("deliberate stdin failure"))
        }
    }

    struct EmptyStdinProvider;

    impl StdinProvider for EmptyStdinProvider {
        fn read_if_piped(&self) -> IoResult<Option<String>> {
            Ok(None)
        }
    }

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn temp_path(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-core-sol-{name}-"))
            .tempdir()
            .expect("tempdir")
    }

    fn write_link_serve_bundle(config: &Path, label: &str) -> LinkServeBundle {
        const CERT: &str = "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n";

        let bundle_dir = config.join("solstone-observer").join("spl").join(label);
        fs::create_dir_all(&bundle_dir).expect("create link serve bundle directory");
        fs::write(bundle_dir.join("private.pem"), "PRIVATE\n").expect("write private key");
        fs::write(bundle_dir.join("cert.pem"), CERT).expect("write client certificate");
        fs::write(bundle_dir.join("chain.pem"), CERT).expect("write certificate chain");
        fs::write(bundle_dir.join("home_attestation.jwt"), "attestation.jwt")
            .expect("write home attestation");
        fs::write(
            bundle_dir.join("peer.json"),
            r#"{"instance_id":"home-instance","home_label":"Home","local_endpoints":[]}"#,
        )
        .expect("write peer metadata");

        LinkServeBundle {
            private_key_pem: "PRIVATE\n".to_string(),
            client_cert_pem: CERT.to_string(),
            ca_chain_pem: vec![CERT.to_string()],
            home_attestation: "attestation.jwt".to_string(),
            instance_id: "home-instance".to_string(),
            home_label: "Home".to_string(),
            endpoints: vec![],
            local_endpoints: json!([]),
        }
    }

    fn observed_wrapper_ports(env: &BTreeMap<String, String>) -> Vec<i64> {
        let ports = std::cell::RefCell::new(Vec::new());
        let factory = |port| {
            ports.borrow_mut().push(port);
            Box::new(ScriptedHttpTransport::new(vec![])) as Box<dyn HttpTransport>
        };
        let pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
        let runner = ScriptedLinkServeRunner::new(vec![]);

        assert_eq!(
            run_dispatched_with_runtime(
                &os_args(&["status"]),
                &[],
                &FailingStdinProvider,
                env,
                &factory,
            ),
            ExitCode::from(EXIT_TEMPFAIL)
        );
        assert_eq!(
            run_top_level_link_with_runtime(
                &os_args(&["link", "join", "--code", "not-a-code"]),
                &os_args(&["join", "--code", "not-a-code"]),
                &FailingStdinProvider,
                env,
                &factory,
                &pairing,
                &runner,
            ),
            ExitCode::from(EXIT_TEMPFAIL)
        );
        pairing.assert_done();
        runner.assert_done();
        ports.into_inner()
    }

    #[test]
    fn resolve_convey_port_defaults_when_unset() {
        assert_eq!(
            resolve_convey_port(&BTreeMap::new()),
            Ok(DEFAULT_CONVEY_PORT)
        );
    }

    #[test]
    fn production_wrappers_construct_the_default_convey_target() {
        assert_eq!(
            observed_wrapper_ports(&BTreeMap::new()),
            vec![DEFAULT_CONVEY_PORT, DEFAULT_CONVEY_PORT]
        );
    }

    #[test]
    fn production_wrappers_construct_the_assigned_convey_target() {
        let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), "6200".to_string())]);
        assert_eq!(observed_wrapper_ports(&env), vec![6200, 6200]);
    }

    #[test]
    fn link_help_returns_before_collecting_the_process_environment() {
        assert_eq!(
            run_top_level_link_with_env_provider(
                &os_args(&["link", "--help"]),
                &os_args(&["--help"]),
                &PanicStdinProvider,
                &|| panic!("link help must not collect the process environment"),
            ),
            ExitCode::SUCCESS
        );
    }

    #[test]
    fn resolve_convey_port_accepts_decimal_range() {
        for (value, expected) in [("6200", 6200), ("1", 1), ("65535", 65535)] {
            let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), value.to_string())]);
            assert_eq!(resolve_convey_port(&env), Ok(expected));
        }
    }

    #[test]
    fn resolve_convey_port_rejects_invalid_explicit_values() {
        for value in ["", "0", "-1", "65536", "abc"] {
            let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), value.to_string())]);
            let output = resolve_convey_port(&env).expect_err("invalid convey port must fail");
            assert_eq!(output.exit, i32::from(EXIT_CONFIG));
            assert!(output.stderr.contains(SOLSTONE_CONVEY_PORT_ENV));
            assert!(output.stderr.contains('1'));
            assert!(output.stderr.contains("65535"));
        }
    }

    #[test]
    fn run_dispatched_with_env_rejects_zero_convey_port_before_runtime_work() {
        let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), "0".to_string())]);
        let stdin = PanicStdinProvider;
        let factory = |_port| -> Box<dyn HttpTransport> {
            panic!("invalid outbound port must fail before transport construction")
        };

        for (all_args, command_args) in [
            (os_args(&["status"]), vec![]),
            (os_args(&["import"]), vec![]),
            (
                os_args(&["call", "journal", "search"]),
                os_args(&["journal", "search"]),
            ),
        ] {
            assert_eq!(
                run_dispatched_with_runtime(&all_args, &command_args, &stdin, &env, &factory,),
                ExitCode::from(EXIT_CONFIG)
            );
        }
    }

    #[test]
    fn run_top_level_link_with_env_rejects_zero_convey_port_before_join() {
        let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), "0".to_string())]);
        let all_args = os_args(&["link", "join", "--code", "not-a-code"]);
        let command_args = os_args(&["join", "--code", "not-a-code"]);
        let factory = |_port| -> Box<dyn HttpTransport> {
            panic!("invalid outbound port must fail before transport construction")
        };
        let pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
        let runner = ScriptedLinkServeRunner::new(vec![]);

        assert_eq!(
            run_top_level_link_with_runtime(
                &all_args,
                &command_args,
                &PanicStdinProvider,
                &env,
                &factory,
                &pairing,
                &runner,
            ),
            ExitCode::from(EXIT_CONFIG)
        );
        pairing.assert_done();
        runner.assert_done();
    }

    #[test]
    fn run_top_level_link_with_env_rejects_zero_convey_port_before_serve() {
        let env = BTreeMap::from([(SOLSTONE_CONVEY_PORT_ENV.to_string(), "0".to_string())]);
        let all_args = os_args(&["link", "serve", "--port", "0"]);
        let command_args = os_args(&["serve", "--port", "0"]);
        let factory = |_port| -> Box<dyn HttpTransport> {
            panic!("invalid outbound port must fail before transport construction")
        };
        let pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
        let runner = ScriptedLinkServeRunner::new(vec![]);

        assert_eq!(
            run_top_level_link_with_runtime(
                &all_args,
                &command_args,
                &PanicStdinProvider,
                &env,
                &factory,
                &pairing,
                &runner,
            ),
            ExitCode::from(EXIT_CONFIG)
        );
        pairing.assert_done();
        runner.assert_done();
    }

    #[test]
    fn link_serve_listener_port_is_independent_of_convey_port() {
        let root = temp_path("convey-port-link-serve");
        let config = root.path().join("config");
        let env = BTreeMap::from([
            (SOLSTONE_CONVEY_PORT_ENV.to_string(), "6200".to_string()),
            ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ]);
        let bundle = write_link_serve_bundle(&config, "laptop");
        let ports = std::cell::RefCell::new(Vec::new());
        let factory = |port| {
            ports.borrow_mut().push(port);
            Box::new(ScriptedHttpTransport::new(vec![])) as Box<dyn HttpTransport>
        };
        let pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
        let runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: LinkServeRequest {
                label: "laptop".to_string(),
                port: 0,
                policy: LinkServeCarrierPolicy::RelayPermitted,
                relay_origin: Some("https://link.solstone.app".to_string()),
                bundle,
            },
            result: Ok(ExpectedLinkServeSession {
                bound_port: 54321,
                serve_result: Ok(()),
            }),
        }]);
        let all_args = os_args(&["link", "serve", "--label", "laptop", "--port", "0"]);
        let command_args = os_args(&["serve", "--label", "laptop", "--port", "0"]);

        assert_eq!(
            run_top_level_link_with_runtime(
                &all_args,
                &command_args,
                &EmptyStdinProvider,
                &env,
                &factory,
                &pairing,
                &runner,
            ),
            ExitCode::SUCCESS
        );
        assert_eq!(ports.into_inner(), vec![6200]);
        pairing.assert_done();
        runner.assert_done();
    }

    #[test]
    fn version_output_matches_restored_native_contract() {
        let output = version_output();
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            format!("solstone {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn buffered_usage_error_output_stays_byte_identical_without_resident_arm() {
        let output = usage_error_output();
        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, USAGE);
        assert_eq!(output.exit, i32::from(EXIT_USAGE));
    }

    #[test]
    fn help_output_matches_restored_root_contract_shape() {
        let output = help_output();
        assert_eq!(output.exit, 0);
        assert_eq!(output.stderr, "");
        assert!(
            output
                .stdout
                .starts_with("solstone - journal access CLI\n\n")
        );
        assert!(
            output
                .stdout
                .contains("Usage: solstone <command> [args...]\n")
        );
        assert!(output.stdout.contains("Apps (solstone call <app>):\n"));
        assert!(output.stdout.contains("  call journal\n"));
        assert!(!output.stdout.contains("Journal: "));
        assert!(!output.stdout.contains("Days: "));
    }

    #[test]
    fn call_help_lists_native_groups_and_journal_compat() {
        let output = help::render_call_root_help();
        assert!(output.contains("Usage: solstone call <app> <verb> [args...]"));
        assert!(output.contains("  activities\n"));
        assert!(output.contains("  journal\n"));
    }

    #[test]
    fn production_link_join_dispatch_supplies_pairing_seam() {
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let clock = SystemClock::default();
        let files = RealFileProvider;
        let args = ["link", "join", "--code", "not-a-code"]
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();

        match dispatch_top_level_link_with_runtime_seams(
            &args,
            TopLevelLinkRuntime {
                env: &env,
                stdin: "",
                today: "20260727",
                transport: &transport,
                clock: &clock,
                files: &files,
                link_pairing: link_join_pairing_seam(),
                link_serve: link_serve_runner(),
            },
        ) {
            LinkDispatch::Buffered(output) => {
                assert_eq!(output.stdout, "");
                assert_eq!(output.exit, 1);
                assert!(!output.stderr.contains("Link pairing seam is unavailable"));
                assert!(
                    output
                        .stderr
                        .contains("Pair code did not match an accepted form")
                );
            }
            LinkDispatch::Resident { .. } => panic!("link join must stay buffered"),
        }
        transport.assert_done();
    }

    #[test]
    fn project_root_resolution_returns_an_existing_directory() {
        assert!(
            resolve_project_root()
                .expect("project root should resolve")
                .is_dir()
        );
    }

    #[test]
    fn project_root_prefers_installed_package_layout_over_checkout_ancestor() {
        let root = temp_path("installed-root");
        let checkout = root.path().join("checkout");
        let bin = checkout.join(".venv").join("bin");
        let site_packages = checkout
            .join(".venv")
            .join("lib")
            .join("python3.13")
            .join("site-packages");
        fs::create_dir_all(checkout.join(".git")).expect("create .git");
        fs::write(checkout.join("pyproject.toml"), "[project]\n").expect("write pyproject");
        write_layout_share(&checkout.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT));
        fs::create_dir_all(site_packages.join("solstone")).expect("create installed package");
        fs::write(site_packages.join("solstone").join("__init__.py"), "").expect("write init");
        fs::create_dir_all(&bin).expect("create bin");

        let resolved = resolve_project_root_from_executable(&bin.join("sol"))
            .expect("installed project root should resolve");
        assert_eq!(resolved, site_packages);
    }

    #[test]
    fn project_root_uses_executable_checkout_ancestry_without_cwd_fallback() {
        let root = temp_path("checkout-root");
        let checkout = root.path().join("checkout");
        let bin = checkout.join("core").join("target").join("debug");
        fs::create_dir_all(checkout.join(".git")).expect("create .git");
        fs::write(checkout.join("pyproject.toml"), "[project]\n").expect("write pyproject");
        // A `solstone` package directory alone is no longer a checkout payload:
        // the payload lives under `core/payload` and is recognised by the same
        // three anchors the installed layout uses.
        fs::create_dir_all(checkout.join("solstone")).expect("create package dir");
        fs::create_dir_all(&bin).expect("create bin");
        assert!(
            resolve_project_root_from_executable(&bin.join("sol")).is_err(),
            "a package directory without the payload must not resolve"
        );

        let payload = checkout.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT);
        write_layout_share(&payload);
        let resolved = resolve_project_root_from_executable(&bin.join("sol"))
            .expect("checkout should resolve");
        assert_eq!(resolved, payload);
    }

    #[test]
    fn project_root_errors_when_executable_artifact_is_unclassified() {
        let root = temp_path("unclassified-root");
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).expect("create bin");
        let error = resolve_project_root_from_executable(&bin.join("sol")).unwrap_err();
        assert!(error.to_string().contains(
            "native solstone project root resolution failed: could not locate source checkout or installed solstone package"
        ));
    }

    fn write_layout_share(share: &Path) {
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().expect("anchor parent")).expect("anchor dir");
            fs::write(&path, relative).expect("anchor");
        }
        fs::create_dir_all(share.join("solstone/talent")).expect("talent");
        fs::create_dir_all(share.join("solstone/apps")).expect("apps");
    }

    #[test]
    fn project_root_resolves_share_layout_and_fails_when_anchor_removed() {
        let root = temp_path("share-layout-root");
        let prefix = root.path().join("tree");
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        write_layout_share(&prefix.join("share"));
        let resolved = resolve_project_root_from_executable(&bin.join("sol"))
            .expect("share layout should resolve");
        assert_eq!(resolved, prefix.join("share"));
        fs::remove_file(
            prefix
                .join("share")
                .join(solstone_core_journal::LAYOUT_BUNDLE_ANCHOR),
        )
        .expect("remove bundle");
        assert!(resolve_project_root_from_executable(&bin.join("sol")).is_err());
    }

    #[test]
    fn service_host_command_moves_to_journal() {
        assert_eq!(
            service_moved_output(OsStr::new("think")),
            CommandOutput::failure(
                "'think' moved to 'journal think' — run that instead.\n('solstone' is the journal-access surface; 'journal' surfaces journal-service commands; see 'journal --help'.)\n",
                SERVICE_MOVED_EXIT,
            )
        );
    }

    #[test]
    fn unknown_command_is_explicitly_unsupported() {
        assert_eq!(
            unsupported_output(),
            CommandOutput::failure(
                "Unsupported native solstone command.\n",
                i32::from(EXIT_USAGE)
            )
        );
    }

    #[test]
    fn invalid_flag_prints_usage() {
        let output = usage_error_output();
        assert_eq!(output.stdout, "");
        assert_eq!(output.exit, i32::from(EXIT_USAGE));
        assert_eq!(output.stderr, "Usage: solstone <command> [args...]\n");
    }

    #[test]
    fn root_help_and_proof_routes_do_not_read_stdin() {
        let provider = PanicStdinProvider;
        for args in [
            vec![],
            os_args(&["-v"]),
            os_args(&["--help"]),
            os_args(&["-h"]),
            os_args(&["help"]),
            os_args(&["--version"]),
            os_args(&["-V"]),
            os_args(&["root"]),
            os_args(&["does-not-exist"]),
            os_args(&["think"]),
            os_args(&["call"]),
            os_args(&["call", "--help"]),
            os_args(&["call", "activities", "--help"]),
            os_args(&["call", "activities", "list", "--help"]),
            os_args(&["import", "--help"]),
        ] {
            let _ = run_with_stdin_provider("sol", args, &provider);
        }
    }

    #[test]
    fn retired_invocations_are_nonzero_without_runtime_side_effects() {
        let provider = PanicStdinProvider;
        for args in [
            os_args(&["--path"]),
            os_args(&["path"]),
            os_args(&["doctor"]),
            os_args(&["check"]),
            os_args(&["call", "journal", "export"]),
            os_args(&["call", "journal", "facet", "doctor"]),
            os_args(&["call", "journal", "facet", "merge"]),
            os_args(&["call", "journal", "merge"]),
            os_args(&["call", "journal"]),
            os_args(&["call", "journal", "facet"]),
            os_args(&["notify", "hello"]),
        ] {
            let outcome = evaluate_args(&args);
            assert!(matches!(outcome, Outcome::Unsupported { .. }));
            assert_ne!(
                run_with_stdin_provider("sol", args, &provider),
                ExitCode::SUCCESS
            );
        }
    }

    #[test]
    fn moved_stub_dispatches_and_exits_two() {
        let args = vec![
            "identity".to_string(),
            "--unknown".to_string(),
            "extra".to_string(),
        ];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let output = dispatch_sol_call_with_seams(
            &args,
            &env,
            "",
            "20260723",
            DispatchSeams {
                transport: &transport,
                clock: None,
                files: None,
                build_identity: None,
                client_item_ids: None,
                notification_sink: None,
            },
        );

        assert_eq!(output.exit, 2);
        assert_eq!(
            output.stderr,
            "Moved to `journal identity` — run that instead.\n"
        );
        transport.assert_done();
    }

    #[test]
    fn call_journal_matches_only_exact_native_leaves() {
        let journal = os_args(&["call", "journal", "search"]);
        let journal_outcome = evaluate_args(&journal);
        assert!(matches!(journal_outcome, Outcome::Migrated { .. }));

        let http_leaf = os_args(&["call", "entities", "search"]);
        let http_outcome = evaluate_args(&http_leaf);
        assert!(matches!(http_outcome, Outcome::Migrated { .. }));

        let unknown = os_args(&["call", "not-real", "list"]);
        let unknown_outcome = evaluate_args(&unknown);
        assert!(matches!(unknown_outcome, Outcome::Unsupported { .. }));

        let partial = os_args(&["call", "journal", "sear"]);
        let partial_outcome = evaluate_args(&partial);
        assert!(matches!(partial_outcome, Outcome::Unsupported { .. }));
    }

    #[test]
    fn retired_contract_is_unsupported_not_compat() {
        assert!(matches!(
            evaluate_args(&os_args(&["contract"])),
            Outcome::Unsupported { .. }
        ));
    }
}
