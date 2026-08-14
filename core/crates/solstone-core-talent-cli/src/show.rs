// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_cogitate::compose_system_instruction;

use crate::CliRun;
use crate::args::{ListOptions, ShowOptions};
use crate::compose::compose_talent;
use crate::discovery::{self, TalentConfig};
use crate::emit;
use crate::inventory;

const PRIORITY_KEYS: &[&str] = &[
    "title",
    "description",
    "schedule",
    "priority",
    "output",
    "tools",
    "hook",
    "color",
];
const LABEL_WIDTH: usize = 14;
const SECTION_WIDTH: usize = 60;
const SECTION_MAX_LINES: usize = 100;

pub(crate) fn run(
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    options: &ShowOptions,
) -> CliRun {
    let resolved = resolve(&options.name, talent_root, apps_root);
    if !resolved.path.is_file() {
        return not_found(&options.name, &resolved.file, options.prompt);
    }
    let parsed = match discovery::read_frontmatter(&resolved.path) {
        Ok(parsed) => parsed,
        Err(error) => return failure(error),
    };
    if !options.prompt {
        let metadata = with_default_color(parsed.metadata);
        return if options.json {
            render_json(&resolved, metadata, parsed.body)
        } else {
            render_default(&resolved, metadata)
        };
    }

    match parsed.metadata.get("type").and_then(Value::as_str) {
        Some("cogitate") => render_cogitate_prompt(
            &resolved,
            parsed.metadata,
            parsed.body,
            talent_root,
            journal_root,
            options,
        ),
        None | Some("prompt") => failure(format!(
            "Prompt '{}' is a hook prompt and cannot be run directly.",
            options.name
        )),
        _ => failure(format!(
            "Prompt context is not available for non-cogitate talents: {}",
            options.name
        )),
    }
}

fn with_default_color(mut metadata: Map<String, Value>) -> Map<String, Value> {
    metadata
        .entry("color".to_owned())
        .or_insert_with(|| Value::String("#6c757d".to_owned()));
    metadata
}

struct ResolvedTalent {
    path: PathBuf,
    file: String,
}

fn resolve(name: &str, talent_root: &Path, apps_root: &Path) -> ResolvedTalent {
    if let Some((app, talent)) = name.split_once(':') {
        ResolvedTalent {
            path: apps_root
                .join(app)
                .join("talent")
                .join(format!("{talent}.md")),
            file: format!("apps/{app}/talent/{talent}.md"),
        }
    } else {
        ResolvedTalent {
            path: talent_root.join(format!("{name}.md")),
            file: format!("talent/{name}.md"),
        }
    }
}

fn not_found(name: &str, file: &str, prompt: bool) -> CliRun {
    let stderr = if prompt {
        format!("Prompt not found: {name}\n")
    } else {
        format!("Prompt not found: {name}\n  looked at: {file}\n")
    };
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

fn failure(message: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{message}\n"),
        exit_code: 1,
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn render_json(resolved: &ResolvedTalent, metadata: Map<String, Value>, body: String) -> CliRun {
    let config = TalentConfig {
        key: resolved.file.trim_end_matches(".md").to_owned(),
        file: resolved.file.clone(),
        metadata,
        body,
    };
    success(emit::jsonl(
        &[config],
        &ListOptions {
            disabled: true,
            ..ListOptions::default()
        },
    ))
}

fn render_default(resolved: &ResolvedTalent, metadata: Map<String, Value>) -> CliRun {
    let mut output = format!("\n{}\n\n", resolved.file);
    let mut printed = Vec::new();
    for key in PRIORITY_KEYS {
        if let Some(value) = metadata.get(*key) {
            write_field(&mut output, key, value);
            printed.push(*key);
        }
    }
    let mut remaining = metadata.keys().collect::<Vec<_>>();
    remaining.sort();
    for key in remaining {
        if !printed.contains(&key.as_str()) && key != "path" && key != "mtime" {
            write_field(&mut output, key, &metadata[key]);
        }
    }

    // A second guarded parse mirrors Python's independent body load: failure omits body-derived lines.
    if let Ok(parsed) = discovery::read_frontmatter(&resolved.path) {
        let variables = scan_variables(&parsed.body);
        if !variables.is_empty() {
            let _ = writeln!(
                output,
                "  {:<LABEL_WIDTH$} {}",
                "variables:",
                variables
                    .iter()
                    .map(|variable| format!("${variable}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        let _ = writeln!(
            output,
            "  {:<LABEL_WIDTH$} {} lines",
            "body:",
            parsed.body.lines().count(),
        );
    }
    output.push('\n');
    success(output)
}

fn write_field(output: &mut String, key: &str, value: &Value) {
    let mut rendered = python_str(value);
    if key == "description" && rendered.chars().count() > 72 {
        rendered = format!("{}...", rendered.chars().take(72).collect::<String>());
    }
    if key == "hook"
        && let Some(post) = value
            .as_object()
            .and_then(|hook| hook.get("post"))
            .and_then(Value::as_str)
            .filter(|post| !post.is_empty())
    {
        rendered = format!("post: {post}");
    }
    let _ = writeln!(output, "  {:<LABEL_WIDTH$} {rendered}", format!("{key}:"));
}

fn python_str(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => python_repr(value),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => value
            .to_string()
            .replace("true", "True")
            .replace("false", "False"),
        Value::Number(value) => value.to_string(),
        Value::String(value) => python_string_repr(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_string_repr(key), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_string_repr(value: &str) -> String {
    let delimiter = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    format!("{delimiter}{}{delimiter}", python_escape(value, delimiter))
}

fn python_escape(value: &str, delimiter: char) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            character if character == delimiter => format!("\\{character}").chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            character => vec![character],
        })
        .collect()
}

fn scan_variables(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut variables = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' || (index > 0 && bytes[index - 1] == b'$') {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        let braced = bytes.get(cursor) == Some(&b'{');
        if braced {
            cursor += 1;
        }
        let start = cursor;
        if !bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            index += 1;
            continue;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if braced && bytes.get(cursor) == Some(&b'}') {
            cursor += 1;
        }
        let variable =
            body[start..cursor - usize::from(braced && bytes[cursor - 1] == b'}')].to_owned();
        if !variables.contains(&variable) {
            variables.push(variable);
        }
        index = cursor;
    }
    variables
}

fn render_cogitate_prompt(
    resolved: &ResolvedTalent,
    metadata: Map<String, Value>,
    body: String,
    talent_root: &Path,
    journal_root: &Path,
    options: &ShowOptions,
) -> CliRun {
    let mut metadata = metadata;
    metadata.insert(
        "path".to_owned(),
        Value::String(resolved.path.display().to_string()),
    );
    let config = TalentConfig {
        key: options.name.clone(),
        file: resolved.file.clone(),
        metadata,
        body,
    };
    let templates_dir = match talent_root.parent() {
        Some(root) => root.join("think/templates"),
        None => {
            return failure(format!(
                "Failed to load talent config: talent root has no parent: {}",
                talent_root.display()
            ));
        }
    };
    let composed = match compose_talent(
        &config,
        journal_root,
        &templates_dir,
        options.facet.as_deref(),
    ) {
        Ok(composed) => composed,
        Err(error) => return failure(format!("Failed to load talent config: {error}")),
    };

    // Intentional divergence: Python crashes with KeyError('model') before rendering real talents.
    let diagnostic = composed
        .get("diagnostic")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let system_instruction = compose_system_instruction(
        diagnostic,
        composed.get("system_instruction").and_then(Value::as_str),
        (!diagnostic).then_some("sol"),
        composed
            .get("read_scope")
            .and_then(Value::as_array)
            .is_some_and(|scope| !scope.is_empty()),
    )
    .unwrap_or_default();
    let access_tier = composed
        .get("access_tier")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let footer = match inventory::tool_surface_line(&composed) {
        Ok(footer) => footer,
        Err(error) => return failure(format!("Failed to load talent config: {error}")),
    };

    let mut output = String::new();
    if options.day.is_some()
        || options.segment.is_some()
        || options.activity.is_some()
        || options.query.is_some()
    {
        output.push_str("Static cogitate prompt view ignores runtime args except --facet.\n");
    }
    let _ = writeln!(
        output,
        "\n  Effective prompt for: {}  tier: {access_tier}",
        options.name
    );
    format_section(
        &mut output,
        "SYSTEM INSTRUCTION",
        &system_instruction,
        options.full,
    );
    // The reference joins transcript / extra_context / user_instruction / prompt into
    // the prompt body; a composed cogitate talent carries only user_instruction, so the
    // body IS that field. It belongs under INSTRUCTION, not folded into the system
    // instruction, which carries the runtime preamble plus the talent's own
    // `system_instruction` and the sol tool hint.
    let instruction = composed
        .get("user_instruction")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format_section(&mut output, "INSTRUCTION", instruction, options.full);
    let _ = writeln!(output, "{footer}");
    output.push('\n');
    success(output)
}

fn format_section(output: &mut String, title: &str, content: &str, full: bool) {
    let _ = writeln!(
        output,
        "\n{}\n  {title}\n{}\n",
        "=".repeat(SECTION_WIDTH),
        "=".repeat(SECTION_WIDTH)
    );
    if content.trim().is_empty() {
        output.push_str("(empty)\n");
        return;
    }
    if full {
        let _ = writeln!(output, "{content}");
        return;
    }
    let (content, omitted) = truncate_content(content);
    let _ = writeln!(output, "{content}");
    if omitted > 0 {
        let _ = writeln!(
            output,
            "\n(use --full to see all {} lines)",
            omitted + SECTION_MAX_LINES
        );
    }
}

fn truncate_content(content: &str) -> (String, usize) {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= SECTION_MAX_LINES {
        return (content.to_owned(), 0);
    }
    let half = SECTION_MAX_LINES / 2;
    let omitted = lines.len() - SECTION_MAX_LINES;
    let mut truncated = lines[..half].join("\n");
    let _ = write!(
        truncated,
        "\n\n... ({omitted} lines omitted)\n{}",
        lines[lines.len() - half..].join("\n")
    );
    (truncated, omitted)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::run_cli;

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("talent")).expect("talent");
        fs::create_dir_all(root.path().join("apps/demo/talent")).expect("app talent");
        fs::create_dir_all(root.path().join("think/templates")).expect("templates");
        root
    }

    fn run(root: &tempfile::TempDir, args: &[&str]) -> CliRun {
        run_cli(
            &args.iter().map(Into::into).collect::<Vec<_>>(),
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            UNIX_EPOCH + Duration::from_secs(1_000),
        )
    }

    #[test]
    fn default_view_orders_formats_and_scans_fields() {
        let root = root();
        fs::write(
            root.path().join("talent/demo.md"),
            concat!(
                "{\n",
                "\"tools\": [\"sol\", true, 5.0, 0.1],\n",
                "\"title\": \"Demo\",\n",
                "\"description\": \"",
                "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuv",
                "\",\n",
                "\"hook\": {\"pre\": \"before\", \"post\": \"after\"},\n",
                "\"zeta\": false\n",
                "}\n",
                "$first ${second} $$hidden $first\n",
            ),
        )
        .expect("prompt");
        let output = run(&root, &["show", "demo"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("  title:         Demo\n"));
        assert!(
            output
                .stdout
                .contains("  tools:         ['sol', True, 5.0, 0.1]\n")
        );
        assert!(output.stdout.contains("  hook:          post: after\n"));
        assert!(output.stdout.contains("  zeta:          False\n"));
        assert!(output.stdout.contains("  variables:     $first, $second\n"));
        assert!(output.stdout.contains("  body:          1 lines\n"));
        assert!(
            output.stdout.find("title:").expect("title")
                < output.stdout.find("tools:").expect("tools")
        );
        assert!(output.stdout.contains(
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrst..."
        ));
    }

    #[test]
    fn number_text_preserves_python_float_boundaries() {
        assert_eq!(python_repr(&json!(5.0)), "5.0");
        assert_eq!(python_repr(&json!(0.1)), "0.1");
    }

    #[test]
    fn chat_pre_hook_uses_python_dict_rendering() {
        let chat = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../solstone/talent/chat.md");
        let parsed = discovery::read_frontmatter(&chat).expect("checked-in chat talent");
        assert_eq!(
            python_str(parsed.metadata.get("hook").expect("hook")),
            "{'pre': 'chat_context'}"
        );
    }

    #[test]
    fn checked_in_read_talent_is_the_cogitate_static_view_fixture() {
        let read = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../solstone/talent/read.md");
        let parsed = discovery::read_frontmatter(&read).expect("checked-in read talent");
        assert_eq!(
            parsed.metadata.get("type"),
            Some(&Value::String("cogitate".to_owned()))
        );
        assert!(parsed.body.contains("## Your Job"));
    }

    #[test]
    fn not_found_distinguishes_plain_and_prompt_paths() {
        let root = root();
        assert_eq!(
            run(&root, &["show", "missing"]),
            CliRun {
                stdout: String::new(),
                stderr: "Prompt not found: missing\n  looked at: talent/missing.md\n".to_owned(),
                exit_code: 1
            }
        );
        assert_eq!(
            run(&root, &["show", "demo:missing", "--prompt"]),
            CliRun {
                stdout: String::new(),
                stderr: "Prompt not found: demo:missing\n".to_owned(),
                exit_code: 1
            }
        );
        assert_eq!(
            run(&root, &["show", "missing", "--prompt"]),
            CliRun {
                stdout: String::new(),
                stderr: "Prompt not found: missing\n".to_owned(),
                exit_code: 1
            }
        );
        assert_eq!(
            run(&root, &["show", "demo:missing"]),
            CliRun {
                stdout: String::new(),
                stderr:
                    "Prompt not found: demo:missing\n  looked at: apps/demo/talent/missing.md\n"
                        .to_owned(),
                exit_code: 1
            }
        );
    }

    #[test]
    fn json_view_uses_the_shared_jsonl_emitter_without_discovery_fields() {
        let root = root();
        fs::write(
            root.path().join("talent/raw.md"),
            "{\n\"title\": \"Raw\"\n}\nbody",
        )
        .expect("raw");
        let output = run(&root, &["show", "raw", "--json"]);
        assert_eq!(
            output.stdout,
            "{\"file\": \"talent/raw.md\", \"title\": \"Raw\", \"color\": \"#6c757d\"}\n"
        );
        assert!(!output.stdout.contains("source"));
        assert!(!output.stdout.contains("mtime"));

        fs::write(
            root.path().join("talent/disabled.md"),
            "{\n\"title\": \"Disabled\",\n\"disabled\": true\n}\nbody",
        )
        .expect("disabled");
        let json = run(&root, &["show", "disabled", "--json"]);
        assert_eq!(json.exit_code, 0, "{}", json.stderr);
        assert!(json.stdout.contains("\"disabled\": true"));
        let default = run(&root, &["show", "disabled"]);
        assert_eq!(default.exit_code, 0, "{}", default.stderr);
        assert!(default.stdout.contains("  disabled:      True\n"));
    }

    #[test]
    fn python_repr_chooses_the_python_string_delimiter() {
        assert_eq!(python_repr(&json!("don't")), r#""don't""#);
        assert_eq!(python_repr(&json!("say \"don't\"")), r#"'say "don\'t"'"#);
    }

    #[test]
    fn prompt_refusals_never_have_a_process_execution_path() {
        let root = root();
        fs::write(root.path().join("talent/hook.md"), "hook body").expect("hook");
        fs::write(
            root.path().join("talent/generate.md"),
            "{\n\"type\": \"generate\"\n}\nbody",
        )
        .expect("generate");
        let hook = run(&root, &["show", "hook", "--prompt"]);
        assert_eq!(
            hook.stderr,
            "Prompt 'hook' is a hook prompt and cannot be run directly.\n"
        );
        let generate = run(&root, &["show", "generate", "--prompt"]);
        assert_eq!(
            generate.stderr,
            "Prompt context is not available for non-cogitate talents: generate\n"
        );
        let source = include_str!("show.rs");
        let command_new = ["Command", "::new"].concat();
        let process_module = ["std::", "process"].concat();
        assert!(!source.contains(&command_new));
        assert!(!source.contains(&process_module));
    }

    #[test]
    fn cogitate_prompt_puts_the_body_under_instruction_and_truncates_it() {
        let root = root();
        let body = (0..105)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            root.path().join("talent/read.md"),
            format!("{{\n\"type\": \"cogitate\",\n\"read_scope\": [\"today\"]\n}}\n{body}"),
        )
        .expect("read");
        let output = run(&root, &["show", "read", "--prompt", "--query", "ignored"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(
            output
                .stdout
                .starts_with("Static cogitate prompt view ignores runtime args except --facet.\n")
        );
        // The two sections carry DIFFERENT things, and asserting only that both
        // headings appear is what let them be swapped: the body was rendered under
        // SYSTEM INSTRUCTION while INSTRUCTION printed "(empty)", and every
        // assertion here still passed.
        let (system, instruction) = output
            .stdout
            .split_once("  INSTRUCTION\n")
            .expect("both sections render");
        assert!(system.contains("SYSTEM INSTRUCTION"));
        assert!(
            !system.contains("line 0"),
            "the talent body must not appear under SYSTEM INSTRUCTION"
        );
        assert!(
            instruction.contains("line 0"),
            "the talent body belongs under INSTRUCTION"
        );
        assert!(
            !output.stdout.contains("(empty)"),
            "a talent with a body must render no empty section"
        );
        assert!(output.stdout.contains("lines omitted)"));
        assert!(output.stdout.contains("(use --full to see all "));
        assert!(output.stdout.contains("tools: "));
        assert!(!output.stdout.contains("model:"));
        let full = run(&root, &["show", "read", "--prompt", "--full"]);
        assert!(full.stdout.contains("line 52"));
        assert!(!full.stdout.contains("lines omitted"));
    }
}
