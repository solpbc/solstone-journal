// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing `journal setup` grammar and resolved setup inputs.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::user_config::{config_path, default_journal, read_user_config};

pub const USAGE: &str = concat!(
    "usage: journal setup [-h] [--journal PATH] [--port INT]\n",
    "                     [--variant {auto,cpu,cuda,coreml}] [--step-timeout-seconds INT]\n",
    "                     [-y] [--dry-run] [--jsonl] [--explain] [--skip-models]\n",
    "                     [--skip-brain] [--skip-skills] [--skip-service] [--skip-wrapper]\n",
    "                     [--skip-path]\n",
    "                     [--accept-existing-journal] [--force] [--clean-uninstall]\n",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupMode {
    Interactive,
    NonInteractive,
    DryRun,
    Explain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupArgs {
    pub journal: Option<PathBuf>,
    pub port: u16,
    pub variant: String,
    pub step_timeout_seconds: i64,
    pub yes: bool,
    pub dry_run: bool,
    pub jsonl: bool,
    pub explain: bool,
    pub skip_models: bool,
    pub skip_brain: bool,
    pub skip_skills: bool,
    pub skip_service: bool,
    pub skip_wrapper: bool,
    pub skip_path: bool,
    pub accept_existing_journal: bool,
    pub force: bool,
    pub clean_uninstall: bool,
    /// Private phase marker used by the archive installer transaction.
    pub(crate) installer_transaction: bool,
    supplied: SuppliedFlags,
}

impl SetupArgs {
    #[must_use]
    pub(crate) fn port_supplied(&self) -> bool {
        self.supplied.port
    }

    #[must_use]
    pub(crate) fn variant_supplied(&self) -> bool {
        self.supplied.variant
    }

    #[must_use]
    pub(crate) fn step_timeout_seconds_supplied(&self) -> bool {
        self.supplied.step_timeout_seconds
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SuppliedFlags {
    port: bool,
    variant: bool,
    step_timeout_seconds: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Parse owner arguments using the process current directory for journal validation.
pub fn parse_args(arguments: &[OsString]) -> Result<SetupArgs, UsageError> {
    let cwd = env::current_dir()
        .map_err(|error| UsageError(format!("could not determine current directory: {error}")))?;
    parse_args_at(arguments, &cwd)
}

/// Parse owner arguments against an explicit current directory.
pub fn parse_args_at(arguments: &[OsString], cwd: &Path) -> Result<SetupArgs, UsageError> {
    // Deliberately do not support argparse-style prefix abbreviations. The reference's
    // `arg_supplied()` cannot see abbreviations, so accepting them could weaken the
    // irreversible `--clean-uninstall` incompatibility checks.
    let mut parsed = SetupArgs {
        journal: None,
        port: 5015,
        variant: "auto".to_owned(),
        step_timeout_seconds: 1800,
        yes: false,
        dry_run: false,
        jsonl: false,
        explain: false,
        skip_models: false,
        skip_brain: false,
        skip_skills: false,
        skip_service: false,
        skip_wrapper: false,
        skip_path: false,
        accept_existing_journal: false,
        force: false,
        clean_uninstall: false,
        installer_transaction: false,
        supplied: SuppliedFlags::default(),
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_os_str();
        let value = |name: &str| -> Result<&OsStr, UsageError> {
            arguments
                .get(index + 1)
                .map(OsString::as_os_str)
                .ok_or_else(|| UsageError(format!("argument {name}: expected one argument")))
        };
        if argument == OsStr::new("--journal") {
            parsed.journal = Some(validate_journal(value("--journal")?, cwd)?);
            index += 1;
        } else if let Some(value) = equals_value(argument, "--journal") {
            parsed.journal = Some(validate_journal(value, cwd)?);
        } else if argument == OsStr::new("--port") {
            parsed.port = parse_port(value("--port")?)?;
            parsed.supplied.port = true;
            index += 1;
        } else if let Some(value) = equals_value(argument, "--port") {
            parsed.port = parse_port(value)?;
            parsed.supplied.port = true;
        } else if argument == OsStr::new("--variant") {
            parsed.variant = parse_variant(value("--variant")?)?;
            parsed.supplied.variant = true;
            index += 1;
        } else if let Some(value) = equals_value(argument, "--variant") {
            parsed.variant = parse_variant(value)?;
            parsed.supplied.variant = true;
        } else if argument == OsStr::new("--step-timeout-seconds") {
            parsed.step_timeout_seconds = parse_integer(value("--step-timeout-seconds")?)?;
            parsed.supplied.step_timeout_seconds = true;
            index += 1;
        } else if let Some(value) = equals_value(argument, "--step-timeout-seconds") {
            parsed.step_timeout_seconds = parse_integer(value)?;
            parsed.supplied.step_timeout_seconds = true;
        } else if matches!(argument, value if value == OsStr::new("-y") || value == OsStr::new("--yes") || value == OsStr::new("--non-interactive"))
        {
            parsed.yes = true;
        } else if argument == OsStr::new("--dry-run") {
            parsed.dry_run = true;
        } else if argument == OsStr::new("--jsonl") {
            parsed.jsonl = true;
        } else if argument == OsStr::new("--explain") {
            parsed.explain = true;
        } else if argument == OsStr::new("--skip-models") {
            parsed.skip_models = true;
        } else if argument == OsStr::new("--skip-brain") {
            parsed.skip_brain = true;
        } else if argument == OsStr::new("--skip-skills") {
            parsed.skip_skills = true;
        } else if argument == OsStr::new("--skip-service") {
            parsed.skip_service = true;
        } else if argument == OsStr::new("--skip-wrapper") {
            parsed.skip_wrapper = true;
        } else if argument == OsStr::new("--skip-path") {
            parsed.skip_path = true;
        } else if argument == OsStr::new("--installer-transaction") {
            parsed.installer_transaction = true;
        } else if argument == OsStr::new("--accept-existing-journal") {
            parsed.accept_existing_journal = true;
        } else if argument == OsStr::new("--force") {
            parsed.force = true;
        } else if argument == OsStr::new("--clean-uninstall") {
            parsed.clean_uninstall = true;
        } else {
            return Err(UsageError("unrecognized arguments".to_owned()));
        }
        index += 1;
    }
    Ok(parsed)
}

fn equals_value<'a>(argument: &'a OsStr, name: &str) -> Option<&'a OsStr> {
    let value = argument.to_str()?;
    value.strip_prefix(&format!("{name}=")).map(OsStr::new)
}

fn validate_journal(value: &OsStr, cwd: &Path) -> Result<PathBuf, UsageError> {
    let value = value
        .to_str()
        .ok_or_else(|| UsageError("--journal could not be resolved: path is not UTF-8".into()))?;
    if value.trim().is_empty() {
        return Err(UsageError("--journal must not be empty".into()));
    }
    let path = PathBuf::from(value);
    let resolved = resolve_path(&path, cwd)
        .map_err(|error| UsageError(format!("--journal could not be resolved: {error}")))?;
    if resolved == normalize_path(cwd) {
        return Err(UsageError("--journal must not be empty".into()));
    }
    Ok(path)
}

fn parse_port(value: &OsStr) -> Result<u16, UsageError> {
    let value = value.to_string_lossy();
    let port = value
        .parse::<u16>()
        .map_err(|_| UsageError(format!("--port must be in 1024-65535 (got {value})")))?;
    if !(1024..=65535).contains(&port) {
        return Err(UsageError(format!(
            "--port must be in 1024-65535 (got {port})"
        )));
    }
    Ok(port)
}

fn parse_integer(value: &OsStr) -> Result<i64, UsageError> {
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| UsageError("argument --step-timeout-seconds: invalid int value".into()))
}

fn parse_variant(value: &OsStr) -> Result<String, UsageError> {
    let value = value.to_string_lossy();
    if matches!(value.as_ref(), "auto" | "cpu" | "cuda" | "coreml") {
        Ok(value.into_owned())
    } else {
        Err(UsageError(format!(
            "argument --variant: invalid choice: {value} (choose from auto, cpu, cuda, coreml)"
        )))
    }
}

/// Resolve Python's mode priority exactly.
#[must_use]
pub fn resolve_mode(args: &SetupArgs, stdin_is_tty: bool, stdout_is_tty: bool) -> SetupMode {
    if args.jsonl {
        SetupMode::NonInteractive
    } else if args.explain {
        SetupMode::Explain
    } else if args.dry_run {
        SetupMode::DryRun
    } else if args.yes {
        SetupMode::NonInteractive
    } else if stdin_is_tty && stdout_is_tty {
        SetupMode::Interactive
    } else {
        SetupMode::NonInteractive
    }
}

#[derive(Debug, Clone)]
pub struct ResolutionContext {
    pub home_dir: PathBuf,
    pub current_dir: PathBuf,
    pub journal_env: Option<String>,
    pub journal_variant_env: Option<String>,
    pub is_source_checkout: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSetup {
    pub journal_path: PathBuf,
    pub journal_source: String,
    pub args_resolved: Map<String, Value>,
}

impl ResolvedSetup {
    /// The run loop must use these resolved values, not [`SetupMode`].
    #[must_use]
    pub fn should_short_circuit(&self) -> bool {
        ["explain", "dry_run"].into_iter().any(|key| {
            self.args_resolved
                .get(key)
                .and_then(Value::as_object)
                .and_then(|value| value.get("value"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    }
}

/// Resolve setup's normal journal precedence: CLI, environment, config, default.
pub fn resolve_setup(args: &SetupArgs, context: &ResolutionContext) -> ResolvedSetup {
    let (journal_path, journal_source) = resolve_journal_path(args, context);
    let skip_brain = args.skip_brain || args.skip_models;
    let skip_brain_source = if args.skip_brain {
        "cli:--skip-brain"
    } else if args.skip_models {
        "cli:--skip-models"
    } else {
        "default"
    };
    let mut values = BTreeMap::new();
    values.insert(
        "journal",
        resolved(json!(journal_path), journal_source.clone()),
    );
    values.insert(
        "port",
        resolved(json!(args.port), source(args.supplied.port)),
    );
    values.insert(
        "step_timeout_seconds",
        resolved(
            json!(args.step_timeout_seconds),
            source(args.supplied.step_timeout_seconds),
        ),
    );
    values.insert(
        "variant",
        resolved(json!(args.variant), source(args.supplied.variant)),
    );
    values.insert("yes", resolved(json!(args.yes), source(args.yes)));
    values.insert("force", resolved(json!(args.force), source(args.force)));
    values.insert(
        "dry_run",
        resolved(json!(args.dry_run), source(args.dry_run)),
    );
    values.insert("jsonl", resolved(json!(args.jsonl), source(args.jsonl)));
    values.insert(
        "explain",
        resolved(json!(args.explain), source(args.explain)),
    );
    values.insert(
        "skip_models",
        resolved(json!(args.skip_models), source(args.skip_models)),
    );
    values.insert("skip_brain", resolved(json!(skip_brain), skip_brain_source));
    values.insert(
        "skip_skills",
        resolved(json!(args.skip_skills), source(args.skip_skills)),
    );
    values.insert(
        "skip_service",
        resolved(json!(args.skip_service), source(args.skip_service)),
    );
    values.insert(
        "skip_wrapper",
        resolved(json!(args.skip_wrapper), source(args.skip_wrapper)),
    );
    values.insert(
        "skip_path",
        resolved(json!(args.skip_path), source(args.skip_path)),
    );
    values.insert(
        "accept_existing_journal",
        resolved(
            json!(args.accept_existing_journal),
            source(args.accept_existing_journal),
        ),
    );
    values.insert(
        "journal_variant_env",
        resolved(json!(context.journal_variant_env), "env"),
    );
    values.insert(
        "is_source_checkout",
        resolved(json!(context.is_source_checkout), "detected"),
    );
    ResolvedSetup {
        journal_path,
        journal_source,
        args_resolved: values
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    }
}

/// Resolve clean uninstall's shorter journal precedence: environment, config, default.
#[must_use]
pub fn resolve_clean_uninstall_journal(context: &ResolutionContext) -> PathBuf {
    if let Some(value) = non_empty(context.journal_env.as_deref()) {
        return resolve_configured_path(value, context);
    }
    let config = read_user_config(&config_path(&context.home_dir));
    if let Some(value) = config
        .get("journal")
        .filter(|value| !value.trim().is_empty())
    {
        return resolve_configured_path(value, context);
    }
    default_journal(&context.home_dir)
}

fn resolve_journal_path(args: &SetupArgs, context: &ResolutionContext) -> (PathBuf, String) {
    if let Some(path) = &args.journal {
        return (
            resolve_configured_path(path.to_string_lossy().as_ref(), context),
            "cli".into(),
        );
    }
    if let Some(value) = non_empty(context.journal_env.as_deref()) {
        return (resolve_configured_path(value, context), "env".into());
    }
    let config = read_user_config(&config_path(&context.home_dir));
    if let Some(value) = config
        .get("journal")
        .filter(|value| !value.trim().is_empty())
    {
        return (resolve_configured_path(value, context), "config".into());
    }
    (default_journal(&context.home_dir), "default".into())
}

fn resolve_configured_path(value: &str, context: &ResolutionContext) -> PathBuf {
    resolve_expanded_path(value, &context.home_dir, &context.current_dir)
}

/// Resolve a configured path with the same tilde and canonicalization contract
/// used by setup's CLI/config path precedence.
#[must_use]
pub(crate) fn resolve_expanded_path(value: &str, home_dir: &Path, current_dir: &Path) -> PathBuf {
    let path = expand_tilde(value, home_dir);
    let fallback = if path.is_absolute() {
        path.clone()
    } else {
        current_dir.join(&path)
    };
    resolve_path(&path, current_dir).unwrap_or_else(|_| normalize_path(&fallback))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

fn source(supplied: bool) -> &'static str {
    if supplied { "cli" } else { "default" }
}

fn resolved(value: Value, source: impl Into<String>) -> Value {
    json!({"value": value, "source": source.into()})
}

fn resolve_path(path: &Path, cwd: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(canonicalize_or_normalize(&path))
        }
        Err(error) => Err(error),
    }
}

/// Match Python's non-strict `Path.resolve()`: canonicalize when possible, otherwise
/// retain an absolute, lexically normalized path for a not-yet-created target.
#[must_use]
pub(crate) fn canonicalize_or_normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| normalize_path(path))
}

fn expand_tilde(value: &str, home_dir: &Path) -> PathBuf {
    if value == "~" {
        home_dir.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home_dir.join(rest)
    } else {
        PathBuf::from(value)
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                result.push(component.as_os_str());
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        ResolutionContext, SetupMode, USAGE, parse_args_at, resolve_clean_uninstall_journal,
        resolve_mode, resolve_setup,
    };
    use crate::user_config::{config_path, write_user_config};
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> ResolutionContext {
        let root = std::env::temp_dir().join(format!(
            "solstone-core-setup-args-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        ResolutionContext {
            home_dir: root.join("home"),
            current_dir: root.join("cwd"),
            journal_env: None,
            journal_variant_env: Some("cuda".into()),
            is_source_checkout: true,
        }
    }

    fn args(values: &[&str], context: &ResolutionContext) -> super::SetupArgs {
        let values = values.iter().map(OsString::from).collect::<Vec<_>>();
        parse_args_at(&values, &context.current_dir).unwrap()
    }

    #[test]
    fn usage_has_the_owner_facing_probe_prefix() {
        assert!(USAGE.starts_with("usage: journal setup [-h] [--journal PATH] [--port INT]"));
    }

    #[test]
    fn jsonl_explain_short_circuits_even_though_mode_is_non_interactive() {
        let context = fixture("jsonl-explain");
        let parsed = args(&["--jsonl", "--explain"], &context);
        assert_eq!(resolve_mode(&parsed, true, true), SetupMode::NonInteractive);
        assert!(resolve_setup(&parsed, &context).should_short_circuit());
    }

    #[test]
    fn jsonl_dry_run_short_circuits_even_though_mode_is_non_interactive() {
        let context = fixture("jsonl-dry-run");
        let parsed = args(&["--jsonl", "--dry-run"], &context);
        assert_eq!(resolve_mode(&parsed, true, true), SetupMode::NonInteractive);
        assert!(resolve_setup(&parsed, &context).should_short_circuit());
    }

    #[test]
    fn mode_priority_matches_the_reference() {
        let context = fixture("modes");
        assert_eq!(
            resolve_mode(&args(&[], &context), true, true),
            SetupMode::Interactive
        );
        assert_eq!(
            resolve_mode(&args(&[], &context), true, false),
            SetupMode::NonInteractive
        );
        assert_eq!(
            resolve_mode(&args(&["--yes"], &context), true, true),
            SetupMode::NonInteractive
        );
        assert_eq!(
            resolve_mode(&args(&["--dry-run"], &context), true, true),
            SetupMode::DryRun
        );
        assert_eq!(
            resolve_mode(&args(&["--explain"], &context), true, true),
            SetupMode::Explain
        );
    }

    #[test]
    fn args_resolved_matches_the_reference_key_value_and_source_contract() {
        let mut context = fixture("resolved");
        context.journal_env = Some("/from-env".into());
        let parsed = args(
            &[
                "--port=5016",
                "--variant",
                "cpu",
                "--step-timeout-seconds",
                "-4",
                "--skip-models",
                "--yes",
            ],
            &context,
        );
        let resolved = resolve_setup(&parsed, &context);
        assert_eq!(resolved.journal_path, PathBuf::from("/from-env"));
        assert_eq!(resolved.journal_source, "env");
        assert_eq!(resolved.args_resolved.len(), 18);
        assert_eq!(resolved.args_resolved["port"]["value"], 5016);
        assert_eq!(resolved.args_resolved["port"]["source"], "cli");
        assert_eq!(resolved.args_resolved["variant"]["value"], "cpu");
        assert_eq!(resolved.args_resolved["step_timeout_seconds"]["value"], -4);
        assert_eq!(resolved.args_resolved["skip_brain"]["value"], true);
        assert_eq!(
            resolved.args_resolved["skip_brain"]["source"],
            "cli:--skip-models"
        );
        assert_eq!(
            resolved.args_resolved["journal_variant_env"]["value"],
            "cuda"
        );
        assert_eq!(
            resolved.args_resolved["journal_variant_env"]["source"],
            "env"
        );
        assert_eq!(
            resolved.args_resolved["is_source_checkout"]["source"],
            "detected"
        );
    }

    #[test]
    fn journal_resolution_has_normal_and_clean_uninstall_precedence() {
        let mut context = fixture("journal");
        fs::create_dir_all(context.current_dir.clone()).unwrap();
        write_user_config(&config_path(&context.home_dir), "/from-config").unwrap();
        let parsed = args(&[], &context);
        assert_eq!(resolve_setup(&parsed, &context).journal_source, "config");
        context.journal_env = Some("/from-env".into());
        assert_eq!(resolve_setup(&parsed, &context).journal_source, "env");
        assert_eq!(
            resolve_clean_uninstall_journal(&context),
            PathBuf::from("/from-env")
        );
        let parsed = args(&["--journal", "/from-cli"], &context);
        assert_eq!(resolve_setup(&parsed, &context).journal_source, "cli");
    }

    #[test]
    fn grammar_rejects_abbreviations_and_validates_port_and_journal() {
        let context = fixture("grammar");
        let abbreviated = vec![OsString::from("--cle")];
        assert!(parse_args_at(&abbreviated, &context.current_dir).is_err());
        let bad_port = vec![OsString::from("--port"), OsString::from("no")];
        assert_eq!(
            parse_args_at(&bad_port, &context.current_dir)
                .unwrap_err()
                .0,
            "--port must be in 1024-65535 (got no)"
        );
        let out_of_range_port = vec![OsString::from("--port"), OsString::from("80")];
        assert_eq!(
            parse_args_at(&out_of_range_port, &context.current_dir)
                .unwrap_err()
                .0,
            "--port must be in 1024-65535 (got 80)"
        );
        let blank = vec![OsString::from("--journal"), OsString::from("  ")];
        assert_eq!(
            parse_args_at(&blank, &context.current_dir).unwrap_err().0,
            "--journal must not be empty"
        );
    }
}
