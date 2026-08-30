// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const BACKUP_CLI_LIB: &str = include_str!("../../../solstone-core-backup-cli/src/lib.rs");
const MAINTENANCE_LIB: &str = include_str!("../../../solstone-core-maintenance/src/lib.rs");
const MAINTENANCE_BACKUP_BODY: &str =
    include_str!("../../../solstone-core-maintenance/src/bodies/backup.rs");

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True if `source` calls `symbol(...)` directly, module-qualified, fully-qualified,
/// or through a `use ... symbol as alias;` import alias.
fn source_calls_forbidden_symbol(source: &str, symbol: &str) -> bool {
    if calls_identifier(source, symbol) {
        return true;
    }
    import_aliases(source, symbol)
        .iter()
        .any(|alias| calls_identifier(source, alias))
}

/// True if `source` contains a call-shaped occurrence of the bare identifier `name`:
/// not preceded by an identifier byte, followed (after optional whitespace) by `(`.
fn calls_identifier(source: &str, name: &str) -> bool {
    let bytes = source.as_bytes();
    let needle = name.as_bytes();
    let mut start = 0;
    while let Some(offset) = source[start..].find(name) {
        let index = start + offset;
        let before_ok = index == 0 || !is_identifier_byte(bytes[index - 1]);
        let after = index + needle.len();
        let after_ok = !bytes
            .get(after)
            .is_some_and(|&byte| is_identifier_byte(byte));
        if before_ok && after_ok && source[after..].trim_start().starts_with('(') {
            return true;
        }
        start = index + 1;
    }
    false
}

/// Extract every alias `symbol` is imported under via `use ...::symbol as alias;`.
fn import_aliases(source: &str, symbol: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for statement in source.split(';') {
        let trimmed = statement.trim();
        if !trimmed
            .lines()
            .any(|line| line.trim_start().starts_with("use "))
        {
            continue;
        }
        let Some(symbol_at) = trimmed.find(symbol) else {
            continue;
        };
        let bytes = trimmed.as_bytes();
        let before_ok = symbol_at == 0 || !is_identifier_byte(bytes[symbol_at - 1]);
        let after = symbol_at + symbol.len();
        let after_ok = !bytes
            .get(after)
            .is_some_and(|&byte| is_identifier_byte(byte));
        if !before_ok || !after_ok {
            continue;
        }
        let rest = trimmed[after..].trim_start();
        let Some(rest) = rest.strip_prefix("as") else {
            continue;
        };
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let alias = rest
            .trim_start()
            .split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .next()
            .unwrap_or_default();
        if !alias.is_empty() {
            aliases.push(alias.to_owned());
        }
    }
    aliases
}

#[test]
fn detects_bare_forbidden_call() {
    assert!(source_calls_forbidden_symbol(
        "record_backup_error(journal, clock, reason)",
        "record_backup_error"
    ));
}

#[test]
fn detects_module_qualified_forbidden_call() {
    assert!(source_calls_forbidden_symbol(
        "engine::run_backup(journal, services)",
        "run_backup"
    ));
}

#[test]
fn detects_fully_qualified_forbidden_call() {
    assert!(source_calls_forbidden_symbol(
        "solstone_core_backup_runtime::record_backup_error(a, b, c)",
        "record_backup_error"
    ));
}

#[test]
fn detects_import_alias_forbidden_call() {
    assert!(source_calls_forbidden_symbol(
        "use solstone_core_backup_runtime::run_backup as legacy_run;\nlet x = legacy_run(journal, services);",
        "run_backup"
    ));
}

#[test]
fn ignores_unrelated_and_near_miss_calls() {
    let source = "resolve_tools(&capability, runner, downloader, dirs);\ncapability.execute(&services);\nbackup_run_result(result);";
    assert!(!source_calls_forbidden_symbol(source, "run_backup"));
    assert!(!source_calls_forbidden_symbol(
        source,
        "record_backup_error"
    ));
}

#[test]
fn backup_entry_crates_do_not_call_legacy_backup_runtime_entrypoints() {
    for (name, source) in [
        ("backup-cli lib.rs", BACKUP_CLI_LIB),
        ("maintenance lib.rs", MAINTENANCE_LIB),
        ("maintenance bodies/backup.rs", MAINTENANCE_BACKUP_BODY),
    ] {
        for symbol in ["run_backup", "record_backup_error"] {
            assert!(
                !source_calls_forbidden_symbol(source, symbol),
                "{name} calls forbidden legacy backup-runtime entrypoint {symbol}"
            );
        }
    }
}
